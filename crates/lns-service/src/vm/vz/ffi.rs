use crate::log;
use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSArray, NSError, NSFileHandle, NSString, NSURL};
use objc2_virtualization::{
    VZDirectoryShare, VZDirectorySharingDeviceConfiguration, VZDiskImageStorageDeviceAttachment,
    VZEntropyDeviceConfiguration, VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration,
    VZLinuxBootLoader, VZMACAddress, VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration,
    VZSerialPortConfiguration, VZSharedDirectory, VZSingleDirectoryShare,
    VZSocketDeviceConfiguration, VZStorageDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtioSocketDeviceConfiguration,
    VZVirtioSocketListener, VZVirtioSocketListenerDelegate, VZVirtualMachine,
    VZVirtualMachineConfiguration, VZVirtualMachineDelegate,
};

use crate::vm::{VmSpec, VmmBackend};

#[derive(Clone, Copy)]
struct VsockDevicePtr(*const VZVirtioSocketDevice);
// SAFETY: Vz dispatch queue serialises every dereference of this pointer.
unsafe impl Send for VsockDevicePtr {}
// SAFETY: Vz dispatch queue serialises every dereference of this pointer.
unsafe impl Sync for VsockDevicePtr {}

#[derive(Clone, Copy)]
struct VmPtr(*const VZVirtualMachine);
// SAFETY: Vz dispatch queue serialises every dereference of this pointer.
unsafe impl Send for VmPtr {}
// SAFETY: Vz dispatch queue serialises every dereference of this pointer.
unsafe impl Sync for VmPtr {}

struct SendClosure<F: FnOnce()>(F);
impl<F: FnOnce()> SendClosure<F> {
    fn new(f: F) -> Self {
        Self(f)
    }
    fn run(self) {
        (self.0)();
    }
}
// SAFETY: the closure runs once on the dispatch queue we hand it to.
unsafe impl<F: FnOnce()> Send for SendClosure<F> {}

pub struct VsockConnector {
    queue: DispatchRetained<DispatchQueue>,
    virtio_dev: VsockDevicePtr,
    vm: VmPtr,
    done_tx: mpsc::Sender<Result<()>>,
}

impl VsockConnector {
    #[cfg(test)]
    pub(crate) fn new_for_testing() -> Self {
        let (done_tx, _done_rx) = mpsc::channel();
        Self {
            queue: DispatchQueue::new("lns.vz.test", DispatchQueueAttr::SERIAL),
            virtio_dev: VsockDevicePtr(std::ptr::null()),
            vm: VmPtr(std::ptr::null()),
            done_tx,
        }
    }

    pub fn request_stop(&self) {
        if self.vm.0.is_null() {
            return;
        }
        let vm_ptr = self.vm;
        let done_tx = self.done_tx.clone();
        let work = SendClosure::new(move || {
            // SAFETY: the VM is leaked alongside the connector, so the pointer stays valid; the queue serialises access.
            let vm: &VZVirtualMachine = unsafe { &*vm_ptr.0 };
            // SAFETY: canStop is messaged on the VM's own queue.
            if !unsafe { vm.canStop() } {
                let _ = done_tx.send(Ok(()));
                return;
            }
            let block = RcBlock::new(move |err: *mut NSError| {
                let result = if err.is_null() {
                    Ok(())
                } else {
                    // SAFETY: Vz passes a +0 NSError; retain to read it.
                    let err = unsafe { Retained::retain(err).expect("retain NSError") };
                    Err(anyhow!("vz stop failed: {}", ns_err(&err)))
                };
                let _ = done_tx.send(result);
            });
            // SAFETY: the block retains its captures; vm is valid on this queue.
            unsafe {
                vm.stopWithCompletionHandler(&block);
            }
        });
        self.queue.exec_async(move || work.run());
    }

    pub async fn connect(&self, port: u32, timeout: Duration) -> Result<std::os::fd::RawFd> {
        super::connect_with(self, port, timeout).await
    }
}

impl super::ConnectOnce for VsockConnector {
    async fn connect_once(&self, port: u32) -> Result<std::os::fd::RawFd> {
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<std::os::fd::RawFd>>();
        let dev_ptr = self.virtio_dev;
        let work = SendClosure::new(move || {
            // SAFETY: `dev_ptr` is leaked alongside the VM in `build_and_start`, so it remains valid for the process lifetime.
            let dev: &VZVirtioSocketDevice = unsafe { &*dev_ptr.0 };
            let tx_cell = Mutex::new(Some(tx));
            let block = RcBlock::new(
                move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                    let tx = match tx_cell.lock().unwrap().take() {
                        Some(tx) => tx,
                        None => return,
                    };
                    if !err.is_null() {
                        // SAFETY: Vz passes a +0 retain on NSError; `Retained::retain` bumps to +1 so we own the reference.
                        let err = unsafe { Retained::retain(err) }.expect("retain NSError");
                        let _ = tx.send(Err(anyhow!(
                            "connectToPort({}) failed: {}",
                            port,
                            ns_err(&err)
                        )));
                        return;
                    }
                    if conn.is_null() {
                        let _ = tx.send(Err(anyhow!(
                            "connectToPort({port}) returned nil connection"
                        )));
                        return;
                    }
                    // SAFETY: completion handler passes a retained pointer; dup before the connection object drops and closes its fd.
                    let conn = unsafe { Retained::retain(conn).expect("retain conn") };
                    // SAFETY: conn is a retained NSObject reference live for this scope.
                    let fd = unsafe { conn.fileDescriptor() };
                    if fd < 0 {
                        let _ = tx.send(Err(anyhow!(
                            "connectToPort({port}) returned fd={fd} (closed)"
                        )));
                        return;
                    }
                    // SAFETY: dup duplicates the conn-owned fd into one we own through `dup_fd`.
                    let dup_fd = unsafe { libc::dup(fd) };
                    if dup_fd < 0 {
                        let _ = tx.send(Err(anyhow::Error::from(std::io::Error::last_os_error())
                            .context("dup vsock fd")));
                        return;
                    }
                    let _ = tx.send(Ok(dup_fd));
                },
            );
            // SAFETY: dev outlives the block; block retains its captures and runs on `queue`.
            unsafe {
                dev.connectToPort_completionHandler(port, &block);
            }
        });
        self.queue.exec_async(move || work.run());
        rx.await.context("connect oneshot dropped")?
    }
}

impl crate::vm::GuestTransport for VsockConnector {
    fn connect(
        &self,
        port: u32,
        timeout: Duration,
    ) -> futures_util::future::BoxFuture<'_, Result<std::os::fd::RawFd>> {
        Box::pin(self.connect(port, timeout))
    }

    fn request_stop(&self) {
        self.request_stop();
    }
}

pub struct VzBackend;

impl VmmBackend for VzBackend {
    fn name(&self) -> &'static str {
        "apple-vz"
    }

    fn run(&self, spec: VmSpec) -> Result<()> {
        run_vz(spec)
    }
}

fn run_vz(spec: VmSpec) -> Result<()> {
    let cmdline = crate::vm::build_kernel_cmdline(
        &spec.exec,
        "hvc0",
        true,
        &spec.content_tag,
        spec.descriptor_sha256.as_deref(),
        spec.debug,
        &spec.volumes,
        &spec.binds,
    );

    let queue = DispatchQueue::new("lns.vz", DispatchQueueAttr::SERIAL);

    let (done_tx, done_rx) = mpsc::channel::<Result<()>>();
    let (start_tx, start_rx) = mpsc::sync_channel::<Result<()>>(1);

    let kernel = spec.kernel.clone();
    let initrd = spec.initrd.clone();
    let content_share = spec.content_share.clone();
    let content_tag = spec.content_tag.clone();
    let composefs_descriptor = spec.composefs_descriptor.clone();
    let upper_disk = spec.upper_disk.clone();
    let volumes = spec.volumes.clone();
    let binds = spec.binds.clone();
    let vsock = spec.vsock;
    let connector_tx = spec.connector_tx;
    let console_fd = spec.console_fd;
    let cpus = spec.cpus;
    let mem_mib = spec.memory_mib;
    let queue_for_vm = queue.clone();
    let queue_for_connector = queue.clone();

    queue.exec_async(move || {
        let inputs = BuildInputs {
            kernel: &kernel,
            initrd: &initrd,
            content_share: &content_share,
            content_tag: &content_tag,
            composefs_descriptor: &composefs_descriptor,
            upper_disk: &upper_disk,
            volumes: &volumes,
            binds: &binds,
            vsock: vsock.as_ref(),
            console_fd,
            cmdline: &cmdline,
            cpus,
            mem_mib,
        };
        match build_and_start(
            &inputs,
            &queue_for_vm,
            done_tx.clone(),
            start_tx.clone(),
            connector_tx,
            queue_for_connector,
        ) {
            Ok(()) => {}
            Err(e) => {
                let _ = start_tx.send(Err(e));
            }
        }
    });

    start_rx
        .recv()
        .context("vz queue closure never reported start status")??;

    done_rx
        .recv()
        .context("vz delegate channel closed before guest stopped")?
}

struct BuildInputs<'a> {
    kernel: &'a Path,
    initrd: &'a Path,
    content_share: &'a Path,
    content_tag: &'a str,
    composefs_descriptor: &'a Path,
    upper_disk: &'a Path,
    volumes: &'a [crate::vm::VolumeAttachment],
    binds: &'a [crate::vm::BindAttachment],
    vsock: Option<&'a crate::vm::VsockChannel>,
    console_fd: std::os::fd::RawFd,
    cmdline: &'a str,
    cpus: u8,
    mem_mib: usize,
}

fn build_and_start(
    inputs: &BuildInputs<'_>,
    queue: &DispatchQueue,
    done_tx: mpsc::Sender<Result<()>>,
    start_tx: mpsc::SyncSender<Result<()>>,
    connector_tx: Option<
        tokio::sync::oneshot::Sender<std::sync::Arc<dyn crate::vm::GuestTransport>>,
    >,
    queue_for_connector: DispatchRetained<DispatchQueue>,
) -> Result<()> {
    // SAFETY: all Vz/Obj-C objects retained by `mem::forget` or kept alive via Retained<_>.
    unsafe {
        let config = build_config(inputs)?;
        config
            .validateWithError()
            .map_err(|e| anyhow!("vz config invalid: {}", ns_err(&e)))?;

        let vm = VZVirtualMachine::initWithConfiguration_queue(
            VZVirtualMachine::alloc(),
            &config,
            queue,
        );

        let connector_done_tx = done_tx.clone();
        let delegate = LnsVzDelegate::new(done_tx);
        let proto: &ProtocolObject<dyn VZVirtualMachineDelegate> =
            ProtocolObject::from_ref(&*delegate);
        vm.setDelegate(Some(proto));

        let socket_devices = vm.socketDevices();
        if socket_devices.count() == 0 {
            bail!("lns: VZVirtualMachine has no socket devices despite config — Vz refused vsock?");
        }
        let abstract_dev = socket_devices.objectAtIndex(0);
        let virtio_dev: Retained<VZVirtioSocketDevice> = Retained::cast_unchecked(abstract_dev);

        let mut listeners_for_leak: Vec<Retained<VZVirtioSocketListener>> = Vec::new();
        let mut delegates_for_leak: Vec<Retained<LnsVsockListenerDelegate>> = Vec::new();

        if let Some(channel) = inputs.vsock {
            install_vsock_listener(
                &virtio_dev,
                channel,
                &mut listeners_for_leak,
                &mut delegates_for_leak,
            );
        }

        let start_tx = Mutex::new(Some(start_tx));
        let block = RcBlock::new(move |err: *mut NSError| {
            let tx = match start_tx.lock().unwrap().take() {
                Some(tx) => tx,
                None => return,
            };
            if err.is_null() {
                let _ = tx.send(Ok(()));
            } else {
                let err = Retained::retain(err).expect("vz start NSError retain");
                let _ = tx.send(Err(anyhow!("vz start failed: {}", ns_err(&err))));
            }
        });
        vm.startWithCompletionHandler(&block);

        let virtio_dev_ptr = Retained::as_ptr(&virtio_dev);
        let vm_ptr = Retained::as_ptr(&vm);

        std::mem::forget(vm);
        std::mem::forget(delegate);
        std::mem::forget(virtio_dev);
        for l in listeners_for_leak {
            std::mem::forget(l);
        }
        for d in delegates_for_leak {
            std::mem::forget(d);
        }

        if let Some(tx) = connector_tx {
            let connector = VsockConnector {
                queue: queue_for_connector,
                virtio_dev: VsockDevicePtr(virtio_dev_ptr),
                vm: VmPtr(vm_ptr),
                done_tx: connector_done_tx,
            };
            let _ = tx.send(std::sync::Arc::new(connector));
        }

        Ok(())
    }
}

unsafe fn build_config(
    inputs: &BuildInputs<'_>,
) -> Result<Retained<VZVirtualMachineConfiguration>> {
    // SAFETY: Vz/Obj-C constructors take ownership of the +1 retained refs we hand them.
    unsafe {
        let kernel_url = file_url(inputs.kernel);
        let boot_loader =
            VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url);
        boot_loader.setCommandLine(&NSString::from_str(inputs.cmdline));
        let initrd_url = file_url(inputs.initrd);
        boot_loader.setInitialRamdiskURL(Some(&initrd_url));

        let console_handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
            NSFileHandle::alloc(),
            inputs.console_fd,
            true,
        );
        let serial_attach =
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                Some(&console_handle),
                Some(&console_handle),
            );
        let console = VZVirtioConsoleDeviceSerialPortConfiguration::new();
        console.setAttachment(Some(&serial_attach));
        let serial_ports: Retained<NSArray<VZSerialPortConfiguration>> =
            NSArray::from_retained_slice(&[Retained::cast_unchecked(console)]);

        let entropy = VZVirtioEntropyDeviceConfiguration::new();
        let entropy_devices: Retained<NSArray<VZEntropyDeviceConfiguration>> =
            NSArray::from_retained_slice(&[Retained::cast_unchecked(entropy)]);

        let nat = VZNATNetworkDeviceAttachment::new();
        let net = VZVirtioNetworkDeviceConfiguration::new();
        net.setAttachment(Some(&nat));
        net.setMACAddress(&VZMACAddress::randomLocallyAdministeredAddress());
        let networks: Retained<NSArray<VZNetworkDeviceConfiguration>> =
            NSArray::from_retained_slice(&[Retained::cast_unchecked(net)]);

        let platform = VZGenericPlatformConfiguration::new();

        let config = VZVirtualMachineConfiguration::new();
        config.setCPUCount(inputs.cpus as usize);
        config.setMemorySize((inputs.mem_mib as u64) * 1024 * 1024);
        config.setBootLoader(Some(&boot_loader));
        config.setSerialPorts(&serial_ports);
        config.setEntropyDevices(&entropy_devices);
        config.setNetworkDevices(&networks);
        config.setPlatform(&platform);

        let vsock_dev = VZVirtioSocketDeviceConfiguration::new();
        let socket_devices: Retained<NSArray<VZSocketDeviceConfiguration>> =
            NSArray::from_retained_slice(&[Retained::cast_unchecked(vsock_dev)]);
        config.setSocketDevices(&socket_devices);

        if !inputs.content_share.exists() {
            bail!(
                "virtiofs share dir does not exist: {}",
                inputs.content_share.display()
            );
        }
        let dir_url = NSURL::fileURLWithPath_isDirectory(
            &NSString::from_str(&inputs.content_share.to_string_lossy()),
            true,
        );
        let shared =
            VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &dir_url, true);
        let single_share: Retained<VZDirectoryShare> = Retained::cast_unchecked(
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared),
        );
        let fs_dev = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &NSString::from_str(inputs.content_tag),
        );
        fs_dev.setShare(Some(&*single_share));
        let mut share_devs: Vec<Retained<VZDirectorySharingDeviceConfiguration>> =
            vec![Retained::cast_unchecked(fs_dev)];
        for (i, bind) in inputs.binds.iter().enumerate() {
            if !bind.host_source.exists() {
                bail!(
                    "host bind source does not exist: {}",
                    bind.host_source.display()
                );
            }
            let bind_url = NSURL::fileURLWithPath_isDirectory(
                &NSString::from_str(&bind.host_source.to_string_lossy()),
                true,
            );
            let bind_shared = VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &bind_url,
                bind.read_only,
            );
            let bind_single: Retained<VZDirectoryShare> =
                Retained::cast_unchecked(VZSingleDirectoryShare::initWithDirectory(
                    VZSingleDirectoryShare::alloc(),
                    &bind_shared,
                ));
            let bind_dev = VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &NSString::from_str(&crate::vm::bind_share_tag(i)),
            );
            bind_dev.setShare(Some(&*bind_single));
            share_devs.push(Retained::cast_unchecked(bind_dev));
        }
        let dirs: Retained<NSArray<VZDirectorySharingDeviceConfiguration>> =
            NSArray::from_retained_slice(&share_devs);
        config.setDirectorySharingDevices(&dirs);

        // Vz preserves array order in setStorageDevices; upper_disk must be first (→ /dev/vda) and composefs_descriptor second (→ /dev/vdb) to match the cmdline keys.
        let upper_url = file_url(inputs.upper_disk);
        let upper_attach = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &upper_url,
            false,
        )
        .map_err(|e| anyhow!("opening upper disk image: {}", ns_err(&e)))?;
        let upper_block = VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &upper_attach,
        );

        let descriptor_url = file_url(inputs.composefs_descriptor);
        let descriptor_attach = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &descriptor_url,
            true,
        )
        .map_err(|e| anyhow!("opening composefs descriptor: {}", ns_err(&e)))?;
        let descriptor_block = VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &descriptor_attach,
        );

        let mut devices: Vec<Retained<VZStorageDeviceConfiguration>> = vec![
            Retained::cast_unchecked(upper_block),
            Retained::cast_unchecked(descriptor_block),
        ];
        for disk in crate::vm::volume_disks(inputs.volumes) {
            let url = file_url(&disk.host_image);
            let attach = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &url,
                false,
            )
            .map_err(|e| {
                anyhow!(
                    "opening volume image {}: {}",
                    disk.host_image.display(),
                    ns_err(&e)
                )
            })?;
            let block = VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attach,
            );
            devices.push(Retained::cast_unchecked(block));
        }
        let storage: Retained<NSArray<VZStorageDeviceConfiguration>> =
            NSArray::from_retained_slice(&devices);
        config.setStorageDevices(&storage);

        Ok(config)
    }
}

unsafe fn install_vsock_listener(
    virtio_dev: &VZVirtioSocketDevice,
    channel: &crate::vm::VsockChannel,
    listeners_for_leak: &mut Vec<Retained<VZVirtioSocketListener>>,
    delegates_for_leak: &mut Vec<Retained<LnsVsockListenerDelegate>>,
) {
    log::debug!(port = channel.port, "installing vsock listener");
    // SAFETY: retained listener+delegate are pushed to the caller's leak vecs.
    unsafe {
        let listener = VZVirtioSocketListener::new();
        let delegate_obj = LnsVsockListenerDelegate::new(channel.fd_tx.clone());
        let listener_proto: &ProtocolObject<dyn VZVirtioSocketListenerDelegate> =
            ProtocolObject::from_ref(&*delegate_obj);
        listener.setDelegate(Some(listener_proto));
        virtio_dev.setSocketListener_forPort(&listener, channel.port);
        listeners_for_leak.push(listener);
        delegates_for_leak.push(delegate_obj);
    }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    if !path.exists() {
        panic!("vz: path does not exist: {}", path.display());
    }
    let s = NSString::from_str(&path.to_string_lossy());
    NSURL::fileURLWithPath(&s)
}

fn ns_err(err: &NSError) -> String {
    err.localizedDescription().to_string()
}

struct DelegateIvars {
    tx: Mutex<Option<mpsc::Sender<Result<()>>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LnsVzDelegate"]
    #[ivars = DelegateIvars]
    struct LnsVzDelegate;

    unsafe impl NSObjectProtocol for LnsVzDelegate {}

    unsafe impl VZVirtualMachineDelegate for LnsVzDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop(&self, _vm: &VZVirtualMachine) {
            self.send(Ok(()));
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn did_stop_with_error(&self, _vm: &VZVirtualMachine, err: &NSError) {
            let msg = ns_err(err);
            self.send(Err(anyhow!("guest stopped with error: {msg}")));
        }
    }
);

impl LnsVzDelegate {
    fn new(tx: mpsc::Sender<Result<()>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            tx: Mutex::new(Some(tx)),
        });
        // SAFETY: NSObject `init` on a fresh `alloc`.
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, r: Result<()>) {
        if let Some(tx) = self.ivars().tx.lock().unwrap().take() {
            let _ = tx.send(r);
        }
    }
}

struct VsockListenerIvars {
    fd_tx: tokio::sync::mpsc::UnboundedSender<std::os::fd::RawFd>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LnsVsockListenerDelegate"]
    #[ivars = VsockListenerIvars]
    struct LnsVsockListenerDelegate;

    unsafe impl NSObjectProtocol for LnsVsockListenerDelegate {}

    unsafe impl VZVirtioSocketListenerDelegate for LnsVsockListenerDelegate {
        #[unsafe(method(listener:shouldAcceptNewConnection:fromSocketDevice:))]
        fn should_accept(
            &self,
            _listener: &VZVirtioSocketListener,
            connection: &VZVirtioSocketConnection,
            _socket_device: &VZVirtioSocketDevice,
        ) -> objc2::runtime::Bool {
            // SAFETY: `connection` is a borrowed Vz NSObject reference live for this call.
            let fd = unsafe { connection.fileDescriptor() };
            if fd < 0 {
                return objc2::runtime::Bool::NO;
            }
            // SAFETY: dup duplicates the connection-owned fd into one we own.
            let dup_fd = unsafe { libc::dup(fd) };
            if dup_fd < 0 {
                return objc2::runtime::Bool::NO;
            }
            if self.ivars().fd_tx.send(dup_fd).is_err() {
                // SAFETY: dup_fd was never handed out; we own it and close on send failure.
                unsafe { libc::close(dup_fd) };
                return objc2::runtime::Bool::NO;
            }
            objc2::runtime::Bool::YES
        }
    }
);

impl LnsVsockListenerDelegate {
    fn new(fd_tx: tokio::sync::mpsc::UnboundedSender<std::os::fd::RawFd>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(VsockListenerIvars { fd_tx });
        // SAFETY: NSObject `init` on a fresh `alloc`.
        unsafe { msg_send![super(this), init] }
    }
}

#[allow(dead_code)]
fn _bail_anchor() {
    fn _f() -> Result<()> {
        bail!("unused");
    }
}
