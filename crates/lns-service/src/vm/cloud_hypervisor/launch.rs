#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::vm::{VmSpec, volume_disks};

pub(crate) struct SocketLayout {
    pub vsock: PathBuf,
    pub api: PathBuf,
    pub virtiofsd: PathBuf,
    pub console_log: PathBuf,
}

impl SocketLayout {
    pub(crate) fn for_run_dir(run_dir: &Path) -> Self {
        Self {
            vsock: run_dir.join("vsock.sock"),
            api: run_dir.join("cloud-hypervisor.sock"),
            virtiofsd: run_dir.join("virtiofsd.sock"),
            console_log: run_dir.join("console.log"),
        }
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

pub(crate) fn virtiofsd_args(virtiofsd_socket: &Path, content_share: &Path) -> Vec<String> {
    vec![
        format!("--socket-path={}", virtiofsd_socket.display()),
        format!("--shared-dir={}", content_share.display()),
        "--cache=never".to_string(),
        "--sandbox=none".to_string(),
    ]
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
    args.push(format!("path={}", spec.upper_disk.display()));
    args.push("--disk".to_string());
    args.push(format!(
        "path={},readonly=on",
        spec.composefs_descriptor.display()
    ));
    for disk in volume_disks(&spec.volumes) {
        args.push("--disk".to_string());
        args.push(format!("path={}", disk.host_image.display()));
    }

    args.push("--fs".to_string());
    args.push(format!(
        "tag={},socket={}",
        spec.content_tag,
        layout.virtiofsd.display()
    ));

    args.push("--vsock".to_string());
    args.push(format!("cid={GUEST_CID},socket={}", layout.vsock.display()));

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{ExecSpec, VolumeAttachment};

    fn spec() -> VmSpec {
        VmSpec {
            run_id: 7,
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
            debug: false,
            exec: ExecSpec::from_image_config(None, &["true".into()]),
            vsock: None,
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
        );
        assert_eq!(
            args,
            vec![
                "--socket-path=/runs/7/virtiofsd.sock".to_string(),
                "--shared-dir=/cache/content".to_string(),
                "--cache=never".to_string(),
                "--sandbox=none".to_string(),
            ]
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
        assert_eq!(disks[0], "path=/cache/runs/7/disk/upper");
        assert_eq!(
            disks[1], "path=/cache/runs/7/descriptor.cfs,readonly=on",
            "the composefs descriptor (vdb) must be read-only"
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
                "path=/cache/runs/7/disk/upper",
                "path=/cache/runs/7/descriptor.cfs,readonly=on",
                "path=/store/a.img",
                "path=/store/b.img",
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
