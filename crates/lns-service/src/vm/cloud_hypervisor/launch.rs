#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::vm::{VmSpec, volume_disks};

pub(crate) struct SocketLayout {
    pub vsock: PathBuf,
    pub api: PathBuf,
    pub virtiofsd: PathBuf,
    pub console_log: PathBuf,
    run_dir: PathBuf,
}

impl SocketLayout {
    pub(crate) fn for_run_dir(run_dir: &Path) -> Self {
        Self {
            vsock: run_dir.join("vsock.sock"),
            api: run_dir.join("cloud-hypervisor.sock"),
            virtiofsd: run_dir.join("virtiofsd.sock"),
            console_log: run_dir.join("console.log"),
            run_dir: run_dir.to_path_buf(),
        }
    }

    pub(crate) fn bind_virtiofsd(&self, index: usize) -> PathBuf {
        self.run_dir.join(format!("virtiofsd-bind-{index}.sock"))
    }
}

pub(crate) const GUEST_CID: u32 = 3;

pub(crate) fn kernel_cmdline(spec: &VmSpec) -> String {
    crate::vm::build_kernel_cmdline(
        &spec.exec,
        "hvc0",
        true,
        &spec.content_tag,
        spec.descriptor_sha256.as_deref(),
        spec.debug,
        &spec.volumes,
        &spec.binds,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BindIdMap {
    pub guest_uid: u32,
    pub guest_gid: u32,
    pub host_uid: u32,
    pub host_gid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShareAccess {
    ReadOnly,
    ReadWrite,
}

pub(crate) fn virtiofsd_args(
    socket: &Path,
    shared_dir: &Path,
    id_map: Option<BindIdMap>,
    access: ShareAccess,
) -> Vec<String> {
    let mut args = vec![
        format!("--socket-path={}", socket.display()),
        format!("--shared-dir={}", shared_dir.display()),
        "--cache=never".to_string(),
    ];
    match id_map {
        Some(m) => {
            args.push("--sandbox=namespace".to_string());
            args.push(format!("--uid-map=:{}:{}:1:", m.guest_uid, m.host_uid));
            args.push(format!("--gid-map=:{}:{}:1:", m.guest_gid, m.host_gid));
        }
        None => args.push("--sandbox=none".to_string()),
    }
    if access == ShareAccess::ReadOnly {
        args.push("--readonly".to_string());
    }
    args
}

pub(crate) fn cloud_hypervisor_args(spec: &VmSpec, layout: &SocketLayout) -> Vec<String> {
    let mut args = vec![
        "--api-socket".to_string(),
        layout.api.display().to_string(),
        "--cpus".to_string(),
        format!("boot={}", spec.cpus),
        "--memory".to_string(),
        format!("size={}M,shared=on", spec.memory_mib),
        "--kernel".to_string(),
        spec.kernel.display().to_string(),
        "--initramfs".to_string(),
        spec.initrd.display().to_string(),
        "--cmdline".to_string(),
        kernel_cmdline(spec),
        "--console".to_string(),
        format!("file={}", layout.console_log.display()),
        "--serial".to_string(),
        "off".to_string(),
    ];

    args.push("--disk".to_string());
    args.push(format!("path={},image_type=raw", spec.upper_disk.display()));
    args.push("--disk".to_string());
    args.push(format!(
        "path={},readonly=on,image_type=raw",
        spec.composefs_descriptor.display()
    ));
    for disk in volume_disks(&spec.volumes) {
        args.push("--disk".to_string());
        args.push(format!("path={},image_type=raw", disk.host_image.display()));
    }

    args.push("--fs".to_string());
    args.push(format!(
        "tag={},socket={}",
        spec.content_tag,
        layout.virtiofsd.display()
    ));
    for i in 0..spec.binds.len() {
        args.push("--fs".to_string());
        args.push(format!(
            "tag={},socket={}",
            crate::vm::bind_share_tag(i),
            layout.bind_virtiofsd(i).display()
        ));
    }

    args.push("--vsock".to_string());
    args.push(format!("cid={GUEST_CID},socket={}", layout.vsock.display()));

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{BindAttachment, ExecSpec, VolumeAttachment};

    fn spec() -> VmSpec {
        VmSpec {
            run_id: "aa07".to_string(),
            cpus: 4,
            memory_mib: 2048,
            kernel: PathBuf::from("/cache/kernel/Image"),
            initrd: PathBuf::from("/cache/initramfs/initramfs.cpio.gz"),
            composefs_descriptor: PathBuf::from("/cache/runs/7/descriptor.cfs"),
            content_share: PathBuf::from("/cache/content"),
            content_tag: "lns-content".into(),
            descriptor_sha256: Some("sha256:deadbeef".into()),
            upper_disk: PathBuf::from("/cache/runs/7/disk/upper"),
            volumes: vec![],
            binds: vec![],
            workload_uid: Some(65534),
            workload_gid: Some(65534),
            debug: false,
            exec: ExecSpec::from_image_config(None, None, &["true".into()]),
            vsock: Vec::new(),
            no_nic: false,
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
        }
    }

    fn layout() -> SocketLayout {
        SocketLayout::for_run_dir(Path::new("/cache/runs/7"))
    }

    fn arg_value<'a>(args: &'a [String], flag: &str) -> &'a str {
        let pos = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("flag {flag} not present in {args:?}"));
        args.get(pos + 1)
            .unwrap_or_else(|| panic!("flag {flag} has no value in {args:?}"))
    }

    #[test]
    fn a_guest_that_asks_for_no_network_device_is_given_no_network_argument() {
        let mut spec = spec();
        spec.no_nic = true;

        let args = cloud_hypervisor_args(&spec, &layout());

        assert!(
            !args.iter().any(|arg| arg.starts_with("--net")),
            "a sidecar's isolation is the absence of the device, not a rule on it: {args:?}"
        );
    }

    #[test]
    fn socket_layout_places_every_endpoint_under_the_run_dir() {
        let l = SocketLayout::for_run_dir(Path::new("/runs/9"));
        assert_eq!(l.vsock, PathBuf::from("/runs/9/vsock.sock"));
        assert_eq!(l.api, PathBuf::from("/runs/9/cloud-hypervisor.sock"));
        assert_eq!(l.virtiofsd, PathBuf::from("/runs/9/virtiofsd.sock"));
        assert_eq!(l.console_log, PathBuf::from("/runs/9/console.log"));
    }

    #[test]
    fn virtiofsd_args_serve_the_content_store_on_its_socket() {
        let args = virtiofsd_args(
            Path::new("/runs/7/virtiofsd.sock"),
            Path::new("/cache/content"),
            None,
            ShareAccess::ReadOnly,
        );
        assert_eq!(
            args,
            vec![
                "--socket-path=/runs/7/virtiofsd.sock".to_string(),
                "--shared-dir=/cache/content".to_string(),
                "--cache=never".to_string(),
                "--sandbox=none".to_string(),
                "--readonly".to_string(),
            ],
            "the content share is world-readable and keeps 1:1 uids — no map"
        );
    }

    #[test]
    fn virtiofsd_args_map_the_workload_ids_to_the_host_user_for_a_bind() {
        let args = virtiofsd_args(
            Path::new("/runs/7/virtiofsd-bind-0.sock"),
            Path::new("/host/proj"),
            Some(BindIdMap {
                guest_uid: 65534,
                guest_gid: 65534,
                host_uid: 1001,
                host_gid: 100,
            }),
            ShareAccess::ReadWrite,
        );
        assert!(
            args.contains(&"--uid-map=:65534:1001:1:".to_string())
                && args.contains(&"--gid-map=:65534:100:1:".to_string()),
            "the workload's guest uid/gid must act as the host user so it reads and writes a host-owned bind, like Vz: {args:?}"
        );
        assert!(
            args.contains(&"--sandbox=namespace".to_string())
                && !args.contains(&"--sandbox=none".to_string()),
            "id maps only take effect under the namespace sandbox: {args:?}"
        );
    }

    #[test]
    fn memory_is_shared_so_vhost_user_fs_can_map_the_guest_region() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert_eq!(arg_value(&args, "--memory"), "size=2048M,shared=on");
    }

    #[test]
    fn cpus_kernel_and_initramfs_come_from_the_spec() {
        let s = spec();
        let args = cloud_hypervisor_args(&s, &layout());
        assert_eq!(arg_value(&args, "--cpus"), "boot=4");
        assert_eq!(arg_value(&args, "--kernel"), "/cache/kernel/Image");
        assert_eq!(
            arg_value(&args, "--initramfs"),
            "/cache/initramfs/initramfs.cpio.gz"
        );
    }

    #[test]
    fn console_captures_hvc0_to_a_file_and_the_legacy_serial_is_off() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert_eq!(
            arg_value(&args, "--console"),
            "file=/cache/runs/7/console.log"
        );
        assert_eq!(arg_value(&args, "--serial"), "off");
        let cmdline = arg_value(&args, "--cmdline");
        assert!(
            cmdline.contains("console=hvc0"),
            "cmdline must select the virtio console: {cmdline:?}"
        );
    }

    #[test]
    fn cmdline_keeps_pci_on_because_cloud_hypervisor_uses_virtio_pci() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert!(
            !arg_value(&args, "--cmdline").contains("pci=off"),
            "CH presents virtio-pci on every arch; pci=off would hide all devices"
        );
    }

    #[test]
    fn disks_are_ordered_upper_then_readonly_descriptor() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        let disks: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--disk" && i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(disks.len(), 2, "no volumes → upper + descriptor only");
        assert_eq!(disks[0], "path=/cache/runs/7/disk/upper,image_type=raw");
        assert_eq!(
            disks[1], "path=/cache/runs/7/descriptor.cfs,readonly=on,image_type=raw",
            "the composefs descriptor (vdb) must be read-only"
        );
    }

    #[test]
    fn every_disk_declares_image_type_raw_so_sector_zero_writes_are_allowed() {
        let mut s = spec();
        s.volumes = vec![VolumeAttachment {
            host_image: "/store/a.img".into(),
            target: "/data".into(),
            read_only: false,
        }];
        let args = cloud_hypervisor_args(&s, &layout());
        let disks: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--disk" && i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert!(
            disks.iter().all(|d| d.contains("image_type=raw")),
            "cloud-hypervisor >=51 disables sector-0 writes on auto-detected raw images, \
             which breaks the guest writing the upper/volume filesystems; declare the type: {disks:?}"
        );
    }

    #[test]
    fn volume_disks_follow_the_descriptor_in_attachment_order_and_dedupe() {
        let mut s = spec();
        s.volumes = vec![
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/data".into(),
                read_only: false,
            },
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/srv".into(),
                read_only: true,
            },
            VolumeAttachment {
                host_image: "/store/b.img".into(),
                target: "/ro".into(),
                read_only: true,
            },
        ];
        let args = cloud_hypervisor_args(&s, &layout());
        let disks: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--disk" && i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(
            disks,
            vec![
                "path=/cache/runs/7/disk/upper,image_type=raw",
                "path=/cache/runs/7/descriptor.cfs,readonly=on,image_type=raw",
                "path=/store/a.img,image_type=raw",
                "path=/store/b.img,image_type=raw",
            ],
            "one writable block device per distinct image, /dev/vdc onward"
        );
    }

    #[test]
    fn fs_device_advertises_the_content_tag_and_virtiofsd_socket() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert_eq!(
            arg_value(&args, "--fs"),
            "tag=lns-content,socket=/cache/runs/7/virtiofsd.sock"
        );
    }

    #[test]
    fn bind_virtiofsd_socket_is_distinct_per_index_under_the_run_dir() {
        let l = SocketLayout::for_run_dir(Path::new("/runs/9"));
        assert_eq!(
            l.bind_virtiofsd(0),
            PathBuf::from("/runs/9/virtiofsd-bind-0.sock")
        );
        assert_eq!(
            l.bind_virtiofsd(1),
            PathBuf::from("/runs/9/virtiofsd-bind-1.sock")
        );
        assert_ne!(l.bind_virtiofsd(0), l.bind_virtiofsd(1));
    }

    #[test]
    fn each_host_bind_is_served_as_its_own_fs_share_after_the_content_share() {
        let mut s = spec();
        s.binds = vec![
            BindAttachment {
                host_source: "/host/proj".into(),
                target: "/work".into(),
                read_only: false,
                dropped_paths: vec![],
            },
            BindAttachment {
                host_source: "/host/data".into(),
                target: "/data".into(),
                read_only: true,
                dropped_paths: vec![],
            },
        ];
        let args = cloud_hypervisor_args(&s, &layout());
        let shares: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--fs" && i + 1 < args.len())
            .map(|(i, _)| &args[i + 1])
            .collect();
        assert_eq!(
            shares,
            vec![
                "tag=lns-content,socket=/cache/runs/7/virtiofsd.sock",
                "tag=lns-bind-0,socket=/cache/runs/7/virtiofsd-bind-0.sock",
                "tag=lns-bind-1,socket=/cache/runs/7/virtiofsd-bind-1.sock",
            ],
            "the content share comes first, then one tagged share per bind in attachment order"
        );
    }

    #[test]
    fn vsock_uses_guest_cid_three_on_the_run_socket() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert_eq!(
            arg_value(&args, "--vsock"),
            "cid=3,socket=/cache/runs/7/vsock.sock"
        );
    }

    #[test]
    fn api_socket_is_wired_for_runtime_control() {
        let args = cloud_hypervisor_args(&spec(), &layout());
        assert_eq!(
            arg_value(&args, "--api-socket"),
            "/cache/runs/7/cloud-hypervisor.sock"
        );
    }
}
