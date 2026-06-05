#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use core::fmt;
use std::ffi::{CStr, CString};
use std::io::ErrorKind;
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;

use crate::cmdline::CmdlineParams;

#[cfg(target_os = "linux")]
mod real;
#[cfg(target_os = "linux")]
pub use real::mount_and_exec;

const PROC: &str = "/proc";
const SYS: &str = "/sys";
const DEV: &str = "/dev";
const DEV_PTS: &str = "/dev/pts";
const DEV_PTMX: &str = "/dev/ptmx";
const CONTENT: &str = "/content";
const COMPOSEFS_META: &str = "/composefs-meta";
const UPPER_MOUNTPOINT: &str = "/mnt/upper";
const NEWROOT: &str = "/newroot";
const INIT_BROKER_PATH: &str = "/init-broker";
const VOLUME_SEED_MOUNT: &str = "/mnt/vol-seed";
const PROC_CMDLINE: &str = "/proc/cmdline";
const CMDLINE_MASK_FILE: &str = "/.lens/.cmdline";
const DEV_FD_LINKS: &[(&str, &str)] = &[
    ("/proc/self/fd", "/dev/fd"),
    ("/proc/self/fd/0", "/dev/stdin"),
    ("/proc/self/fd/1", "/dev/stdout"),
    ("/proc/self/fd/2", "/dev/stderr"),
];
const RUN: &str = "/run";
const RUN_LOCK: &str = "/run/lock";
const RUN_TMPFS_OPTS: &str = "mode=0755,size=64m";
const RUN_LOCK_TMPFS_OPTS: &str = "mode=1777,size=4m";
const UNPRIV_PORT_START_SYSCTL: &str = "/proc/sys/net/ipv4/ip_unprivileged_port_start";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MountFlags {
    None,
    ReadOnly,
    Bind,
    Tmpfs,
    TmpfsNoExec,
}

pub(crate) struct SandboxUser {
    pub name: String,
    pub uid: u32,
}

pub(crate) trait Syscalls {
    fn mount(
        &self,
        source: &CStr,
        target: &CStr,
        fstype: &CStr,
        flags: MountFlags,
        data: Option<&CStr>,
    ) -> std::io::Result<()>;
    fn move_mount(&self, from: &CStr, to: &CStr) -> std::io::Result<()>;
    fn mkdir(&self, path: &CStr, mode: u32) -> std::io::Result<()>;
    fn symlink(&self, target: &CStr, linkpath: &CStr) -> std::io::Result<()>;
    fn mknod_char(&self, path: &CStr, mode: u32, major: u32, minor: u32) -> std::io::Result<()>;
    fn chroot(&self, path: &CStr) -> std::io::Result<()>;
    fn chdir(&self, path: &CStr) -> std::io::Result<()>;
    fn path_exists(&self, path: &CStr) -> bool;
    fn open_ro(&self, path: &CStr) -> std::io::Result<RawFd>;
    fn fexecve(&self, fd: RawFd, argv0: &CStr) -> std::io::Error;
    fn lchown(&self, path: &CStr, uid: u32, gid: u32) -> std::io::Result<()>;
    fn umount(&self, target: &CStr) -> std::io::Result<()>;
    fn write_root_file(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()>;
    fn write_sysctl(&self, path: &str, value: &str) -> std::io::Result<()>;
    fn seed_pristine_volume(&self, seed_mount: &str, image_target: &str) -> Result<(), MountError>;
    fn verify_descriptor_digest(
        &self,
        device_path: &str,
        expected_hex: &str,
    ) -> Result<(), MountError>;
}

#[derive(Debug)]
pub enum MountError {
    MissingCmdlineKey(&'static str),
    IncompleteVolume(usize),
    InteriorNul(&'static str),
    Syscall { op: String, err: std::io::Error },
    DescriptorDigestMismatch { expected: String, actual: String },
    DescriptorRead(std::io::Error),
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCmdlineKey(k) => {
                write!(f, "required kernel cmdline key missing: {k}")
            }
            Self::IncompleteVolume(idx) => write!(
                f,
                "volume.{idx} on the kernel cmdline is missing its dev or target \
                 (likely a truncated cmdline); refusing to boot rather than start \
                 the workload without its volume"
            ),
            Self::InteriorNul(field) => {
                write!(f, "cmdline field {field} contained an interior NUL")
            }
            Self::Syscall { op, err } => write!(f, "{op}: {err}"),
            Self::DescriptorDigestMismatch { expected, actual } => write!(
                f,
                "composefs descriptor sha256 mismatch — expected {expected}, got {actual}; \
                 refusing to mount (anti-tamper guard)"
            ),
            Self::DescriptorRead(err) => {
                write!(
                    f,
                    "reading composefs descriptor device for digest check: {err}"
                )
            }
        }
    }
}

impl std::error::Error for MountError {}

pub fn overlay_options() -> String {
    format!(
        "lowerdir={COMPOSEFS_META}::{CONTENT},\
         upperdir={UPPER_MOUNTPOINT}/upper,\
         workdir={UPPER_MOUNTPOINT}/work,\
         redirect_dir=on,metacopy=on"
    )
}

pub fn verify_descriptor_digest(device_path: &str, expected_hex: &str) -> Result<(), MountError> {
    use sha2::{Digest, Sha256};
    use std::fs::File;
    use std::io::Read;
    let mut f = File::open(device_path).map_err(MountError::DescriptorRead)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(MountError::DescriptorRead)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_encode(&hasher.finalize());
    if !ct_eq(actual.as_bytes(), expected_hex.as_bytes()) {
        return Err(MountError::DescriptorDigestMismatch {
            expected: expected_hex.to_string(),
            actual,
        });
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(crate) fn resolve_sandbox_user(
    env_get: impl Fn(&str) -> Option<String>,
) -> Option<SandboxUser> {
    let name = env_get("SANDBOX_USER")?;
    if name.is_empty() {
        return None;
    }
    let uid: u32 = env_get("SANDBOX_UID")
        .and_then(|s| s.parse().ok())
        .unwrap_or(65534);
    Some(SandboxUser { name, uid })
}

pub(crate) fn write_sandbox_user_if_missing(newroot: &str, user: &SandboxUser, sys: &dyn Syscalls) {
    let etc = format!("{newroot}/etc");
    let _ = std::fs::create_dir_all(&etc);

    let passwd = format!("{etc}/passwd");
    append_user_line(&passwd, &user.name, user.uid);
    let group = format!("{etc}/group");
    append_group_line(&group, &user.name, user.uid);

    let home = format!("{newroot}/home/{}", user.name);
    match CString::new(home.as_str()) {
        Ok(home_c) => {
            if std::fs::create_dir_all(&home).is_ok() {
                let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755));
                if let Err(err) = sys.lchown(&home_c, user.uid, user.uid) {
                    eprintln!(
                        "lns-init: lchown({home:?}, {}, {}) failed: {err}",
                        user.uid, user.uid
                    );
                }
            }
        }
        Err(e) => {
            eprintln!(
                "lns-init: skipping home setup for {home:?} — path contains interior NUL: {e}"
            );
        }
    }
}

fn append_user_line(passwd_path: &str, user: &str, uid: u32) {
    let existing = std::fs::read_to_string(passwd_path).unwrap_or_default();
    let prefix = format!("{user}:");
    if existing.lines().any(|l| l.starts_with(&prefix)) {
        return;
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!(
        "{user}:x:{uid}:{uid}:lens sandbox user:/home/{user}:/bin/sh\n"
    ));
    let _ = std::fs::write(passwd_path, updated);
}

fn append_group_line(group_path: &str, group: &str, gid: u32) {
    let existing = std::fs::read_to_string(group_path).unwrap_or_default();
    let prefix = format!("{group}:");
    if existing.lines().any(|l| l.starts_with(&prefix)) {
        return;
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!("{group}:x:{gid}:\n"));
    let _ = std::fs::write(group_path, updated);
}

pub fn validate_cmdline(p: &CmdlineParams) -> Result<(), MountError> {
    p.upper_dev
        .as_deref()
        .ok_or(MountError::MissingCmdlineKey("upper.dev"))?;
    p.composefs_descriptor_dev
        .as_deref()
        .ok_or(MountError::MissingCmdlineKey("composefs.descriptor.dev"))?;
    p.content_tag
        .as_deref()
        .ok_or(MountError::MissingCmdlineKey("content.tag"))?;
    if let Some(&idx) = p.incomplete_volumes.first() {
        return Err(MountError::IncompleteVolume(idx));
    }
    Ok(())
}

fn cstring(s: &str, field: &'static str) -> Result<CString, MountError> {
    CString::new(s).map_err(|_| MountError::InteriorNul(field))
}

fn do_mount(
    sys: &dyn Syscalls,
    source: &str,
    target: &str,
    fstype: &str,
    flags: MountFlags,
    data: Option<&str>,
) -> Result<(), MountError> {
    let source_c = cstring(source, "source")?;
    let target_c = cstring(target, "target")?;
    let fstype_c = cstring(fstype, "fstype")?;
    let data_c = match data {
        Some(s) => Some(cstring(s, "data")?),
        None => None,
    };
    sys.mount(&source_c, &target_c, &fstype_c, flags, data_c.as_deref())
        .map_err(|err| MountError::Syscall {
            op: format!("mount({source}, {target}, {fstype})"),
            err,
        })
}

fn do_mount_pseudo(sys: &dyn Syscalls, target: &str, fstype: &str) -> Result<(), MountError> {
    do_mount(sys, "none", target, fstype, MountFlags::None, None)
}

fn do_move_mount(sys: &dyn Syscalls, from: &str, to: &str) -> Result<(), MountError> {
    let from_c = cstring(from, "from")?;
    let to_c = cstring(to, "to")?;
    sys.move_mount(&from_c, &to_c)
        .map_err(|err| MountError::Syscall {
            op: format!("mount --move({from} -> {to})"),
            err,
        })
}

fn do_mkdir(sys: &dyn Syscalls, path: &str, mode: u32) -> Result<(), MountError> {
    let c = cstring(path, "mkdir-path")?;
    match sys.mkdir(&c, mode) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(MountError::Syscall {
            op: format!("mkdir({path})"),
            err,
        }),
    }
}

fn do_mkdir_p(sys: &dyn Syscalls, path: &str, mode: u32) -> Result<(), MountError> {
    let mut acc = String::with_capacity(path.len());
    for component in path.split('/').filter(|c| !c.is_empty()) {
        acc.push('/');
        acc.push_str(component);
        do_mkdir(sys, &acc, mode)?;
    }
    Ok(())
}

fn allow_unprivileged_low_ports(sys: &dyn Syscalls) -> Result<(), MountError> {
    sys.write_sysctl(UNPRIV_PORT_START_SYSCTL, "0")
        .map_err(|err| MountError::Syscall {
            op: format!("write {UNPRIV_PORT_START_SYSCTL}"),
            err,
        })
}

fn do_lchown(sys: &dyn Syscalls, path: &str, uid: u32) -> Result<(), MountError> {
    let c = cstring(path, "lchown-path")?;
    sys.lchown(&c, uid, uid).map_err(|err| MountError::Syscall {
        op: format!("lchown({path}, {uid})"),
        err,
    })
}

fn do_umount(sys: &dyn Syscalls, target: &str) -> Result<(), MountError> {
    let c = cstring(target, "umount-target")?;
    sys.umount(&c).map_err(|err| MountError::Syscall {
        op: format!("umount({target})"),
        err,
    })
}

fn mount_volumes(
    sys: &dyn Syscalls,
    volumes: &[crate::cmdline::VolumeParam],
    newroot: &str,
) -> Result<(), MountError> {
    let mut seeded: Vec<&str> = Vec::new();
    for vol in volumes {
        let target = format!("{newroot}{}", vol.target);
        if !seeded.contains(&vol.dev.as_str()) {
            seed_volume_if_pristine(sys, &vol.dev, &target)?;
            seeded.push(&vol.dev);
        }
        do_mkdir_p(sys, &target, 0o755)?;
        let flags = match vol.read_only {
            true => MountFlags::ReadOnly,
            false => MountFlags::None,
        };
        do_mount(sys, &vol.dev, &target, "ext4", flags, None)?;
    }
    Ok(())
}

fn mount_run_tmpfs(
    sys: &dyn Syscalls,
    newroot: &str,
    sandbox_user: Option<&SandboxUser>,
) -> Result<(), MountError> {
    let run = format!("{newroot}{RUN}");
    do_mkdir(sys, &run, 0o755)?;
    do_mount(
        sys,
        "tmpfs",
        &run,
        "tmpfs",
        MountFlags::Tmpfs,
        Some(RUN_TMPFS_OPTS),
    )?;
    if let Some(user) = sandbox_user {
        do_lchown(sys, &run, user.uid)?;
    }
    let lock = format!("{newroot}{RUN_LOCK}");
    do_mkdir(sys, &lock, 0o755)?;
    do_mount(
        sys,
        "tmpfs",
        &lock,
        "tmpfs",
        MountFlags::TmpfsNoExec,
        Some(RUN_LOCK_TMPFS_OPTS),
    )
}

fn seed_volume_if_pristine(
    sys: &dyn Syscalls,
    dev: &str,
    image_target: &str,
) -> Result<(), MountError> {
    do_mkdir(sys, VOLUME_SEED_MOUNT, 0o755).ok();
    do_mount(sys, dev, VOLUME_SEED_MOUNT, "ext4", MountFlags::None, None)?;
    let result = sys.seed_pristine_volume(VOLUME_SEED_MOUNT, image_target);
    do_umount(sys, VOLUME_SEED_MOUNT)?;
    result
}

fn mask_proc_cmdline(
    sys: &dyn Syscalls,
    newroot: &str,
    raw_cmdline: &str,
) -> Result<(), MountError> {
    let sanitized = crate::cmdline::sanitize_cmdline(raw_cmdline);
    let mask_file = format!("{newroot}{CMDLINE_MASK_FILE}");
    sys.write_root_file(&mask_file, format!("{sanitized}\n").as_bytes(), 0o600)
        .map_err(|err| MountError::Syscall {
            op: format!("write masked cmdline ({mask_file})"),
            err,
        })?;
    let target = format!("{newroot}{PROC_CMDLINE}");
    do_mount(sys, &mask_file, &target, "none", MountFlags::Bind, None)
}

fn ensure_dev_fd_links(sys: &dyn Syscalls) -> Result<(), MountError> {
    for (target, link) in DEV_FD_LINKS {
        let target_c = cstring(target, "devfd-target")?;
        let link_c = cstring(link, "devfd-link")?;
        match sys.symlink(&target_c, &link_c) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(MountError::Syscall {
                    op: format!("symlink({link} -> {target})"),
                    err,
                });
            }
        }
    }
    Ok(())
}

fn ensure_dev_ptmx(sys: &dyn Syscalls) -> Result<(), MountError> {
    let target_c = cstring("/dev/pts/ptmx", "symlink-target")?;
    let link_c = cstring(DEV_PTMX, "symlink-linkpath")?;
    match sys.symlink(&target_c, &link_c) {
        Ok(()) => Ok(()),
        Err(sym_err) => {
            let ptmx_c = cstring(DEV_PTMX, "mknod-path")?;
            sys.mknod_char(&ptmx_c, 0o666, 5, 2)
                .map_err(|mknod_err| MountError::Syscall {
                    op: format!(
                        "ensure {DEV_PTMX} (symlink fallback to mknod both failed: \
                         symlink={sym_err}, mknod={mknod_err})"
                    ),
                    err: mknod_err,
                })
        }
    }
}

fn do_chdir(sys: &dyn Syscalls, path: &str) -> Result<(), MountError> {
    let c = cstring(path, "chdir-path")?;
    sys.chdir(&c).map_err(|err| MountError::Syscall {
        op: format!("chdir({path})"),
        err,
    })
}

fn do_chroot(sys: &dyn Syscalls, path: &str) -> Result<(), MountError> {
    let c = cstring(path, "chroot-path")?;
    sys.chroot(&c).map_err(|err| MountError::Syscall {
        op: format!("chroot({path})"),
        err,
    })
}

fn open_broker_fd(sys: &dyn Syscalls, path_c: &CStr) -> Result<RawFd, MountError> {
    sys.open_ro(path_c).map_err(|err| MountError::Syscall {
        op: format!("open({INIT_BROKER_PATH}, O_RDONLY)"),
        err,
    })
}

fn exec_broker(sys: &dyn Syscalls, broker_fd: RawFd, argv0: &CStr) -> MountError {
    MountError::Syscall {
        op: format!("fexecve(broker_fd for {INIT_BROKER_PATH})"),
        err: sys.fexecve(broker_fd, argv0),
    }
}

#[allow(clippy::cognitive_complexity)] // linear boot sequence: each step is a numbered milestone
fn mount_composefs_and_exec_broker_inner(
    params: &CmdlineParams,
    sandbox_user: Option<&SandboxUser>,
    newroot: &str,
    raw_cmdline: &str,
    sys: &dyn Syscalls,
) -> Result<std::convert::Infallible, MountError> {
    do_mount_pseudo(sys, SYS, "sysfs")?;
    do_mount_pseudo(sys, DEV, "devtmpfs")?;

    do_mkdir(sys, DEV_PTS, 0o755)?;
    do_mount(
        sys,
        "devpts",
        DEV_PTS,
        "devpts",
        MountFlags::None,
        Some("gid=5,mode=620,ptmxmode=666"),
    )?;
    let ptmx_c = cstring(DEV_PTMX, "ptmx-path")?;
    if !sys.path_exists(&ptmx_c) {
        ensure_dev_ptmx(sys)?;
    }
    ensure_dev_fd_links(sys)?;

    let content_tag = params.content_tag.as_deref().unwrap();
    do_mount(
        sys,
        content_tag,
        CONTENT,
        "virtiofs",
        MountFlags::None,
        None,
    )?;

    let descriptor_dev = params.composefs_descriptor_dev.as_deref().unwrap();
    if let Some(expected_hex) = params.composefs_descriptor_sha256.as_deref() {
        sys.verify_descriptor_digest(descriptor_dev, expected_hex)?;
    }
    do_mount(
        sys,
        descriptor_dev,
        COMPOSEFS_META,
        "erofs",
        MountFlags::ReadOnly,
        None,
    )?;

    let upper_dev = params.upper_dev.as_deref().unwrap();
    do_mount(
        sys,
        upper_dev,
        UPPER_MOUNTPOINT,
        "ext4",
        MountFlags::None,
        None,
    )?;

    let upper_upper = format!("{UPPER_MOUNTPOINT}/upper");
    let upper_work = format!("{UPPER_MOUNTPOINT}/work");
    do_mkdir(sys, &upper_upper, 0o755)?;
    do_mkdir(sys, &upper_work, 0o755)?;

    let opts = overlay_options();
    do_mount(
        sys,
        "overlay",
        newroot,
        "overlay",
        MountFlags::None,
        Some(&opts),
    )?;

    mount_volumes(sys, &params.volumes, newroot)?;

    mount_run_tmpfs(sys, newroot, sandbox_user)?;

    allow_unprivileged_low_ports(sys)?;

    let broker_path_c = cstring(INIT_BROKER_PATH, "broker-path")?;
    let broker_fd = open_broker_fd(sys, &broker_path_c)?;

    let newroot_proc = format!("{newroot}{PROC}");
    let newroot_sys = format!("{newroot}{SYS}");
    let newroot_dev = format!("{newroot}{DEV}");
    do_mkdir(sys, &newroot_proc, 0o755).ok();
    do_mkdir(sys, &newroot_sys, 0o755).ok();
    do_mkdir(sys, &newroot_dev, 0o755).ok();
    do_move_mount(sys, PROC, &newroot_proc)?;
    do_move_mount(sys, SYS, &newroot_sys)?;
    do_move_mount(sys, DEV, &newroot_dev)?;

    mask_proc_cmdline(sys, newroot, raw_cmdline)?;

    if let Some(user) = sandbox_user {
        write_sandbox_user_if_missing(newroot, user, sys);
    }

    do_chdir(sys, newroot)?;
    do_move_mount(sys, ".", "/")?;
    do_chroot(sys, ".")?;
    do_chdir(sys, "/")?;

    Err(exec_broker(sys, broker_fd, &broker_path_c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn make_descriptor_file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::TempDir::new().expect("tempdir");
        let path = d.path().join("descriptor.erofs");
        std::fs::write(&path, bytes).expect("write");
        (d, path)
    }

    #[test]
    fn verify_descriptor_digest_accepts_matching_hash() {
        let bytes = b"fake descriptor bytes";
        let (d, path) = make_descriptor_file(bytes);
        let expected = hex_encode(&sha2::Sha256::digest(bytes));
        verify_descriptor_digest(path.to_str().unwrap(), &expected).unwrap();
        drop(d);
    }

    #[test]
    fn verify_descriptor_digest_rejects_mismatch() {
        let (d, path) = make_descriptor_file(b"actual bytes");
        let wrong = "0".repeat(64);
        let err = verify_descriptor_digest(path.to_str().unwrap(), &wrong).unwrap_err();
        assert!(
            matches!(&err, MountError::DescriptorDigestMismatch { expected, actual } if expected == &wrong && expected != actual),
            "got {err:?}"
        );
        drop(d);
    }

    #[test]
    fn verify_descriptor_digest_reports_missing_file() {
        let err = verify_descriptor_digest("/does/not/exist/nope", &"0".repeat(64)).unwrap_err();
        assert!(matches!(err, MountError::DescriptorRead(_)), "got {err:?}");
    }

    #[test]
    fn verify_descriptor_handles_large_file_in_chunks() {
        let bytes: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let (d, path) = make_descriptor_file(&bytes);
        let expected = hex_encode(&sha2::Sha256::digest(&bytes));
        verify_descriptor_digest(path.to_str().unwrap(), &expected).unwrap();
        drop(d);
    }

    #[test]
    fn hex_encode_round_trip() {
        assert_eq!(hex_encode(&[0xAB, 0xCD]), "abcd");
        assert_eq!(hex_encode(&[0x00, 0xFF]), "00ff");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn ct_eq_is_length_sensitive() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn overlay_options_uses_data_only_lower_syntax() {
        let opts = overlay_options();
        assert!(
            opts.contains("/composefs-meta::/content"),
            "missing `::` data-only-lower syntax: {opts}"
        );
    }

    #[test]
    fn overlay_options_enables_redirect_dir_and_metacopy() {
        let opts = overlay_options();
        assert!(opts.contains("redirect_dir=on"));
        assert!(opts.contains("metacopy=on"));
    }

    #[test]
    fn overlay_options_carries_upper_and_work() {
        let opts = overlay_options();
        assert!(opts.contains("upperdir=/mnt/upper/upper"));
        assert!(opts.contains("workdir=/mnt/upper/work"));
    }

    #[test]
    fn validate_cmdline_ok_with_all_keys() {
        let p = CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
        };
        assert!(validate_cmdline(&p).is_ok());
    }

    #[test]
    fn validate_cmdline_complains_when_upper_missing() {
        let p = CmdlineParams {
            upper_dev: None,
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("upper.dev"));
    }

    #[test]
    fn validate_cmdline_complains_when_content_tag_missing() {
        let p = CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: None,
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("content.tag"));
    }

    #[test]
    fn validate_cmdline_complains_when_descriptor_dev_missing() {
        let p = CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: None,
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("composefs.descriptor.dev"));
    }

    #[test]
    fn validate_cmdline_refuses_to_boot_when_a_volume_was_truncated_off_the_cmdline() {
        let p = CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![3],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("volume.3"), "{err}");
    }

    #[test]
    fn init_broker_path_is_at_initramfs_root() {
        assert_eq!(super::INIT_BROKER_PATH, "/init-broker");
    }

    #[test]
    fn mount_error_display_covers_every_variant() {
        let nul = MountError::InteriorNul("source");
        assert!(format!("{nul}").contains("interior NUL"));
        let missing = MountError::MissingCmdlineKey("upper.dev");
        assert!(format!("{missing}").contains("upper.dev"));
        let incomplete = MountError::IncompleteVolume(3);
        assert!(format!("{incomplete}").contains("volume.3"));
        let read = MountError::DescriptorRead(std::io::Error::other("boom"));
        assert!(format!("{read}").contains("composefs descriptor"));
        let syscall = MountError::Syscall {
            op: "mount(x)".into(),
            err: std::io::Error::other("nope"),
        };
        assert!(format!("{syscall}").contains("mount(x): nope"));
        let mismatch = MountError::DescriptorDigestMismatch {
            expected: "aa".into(),
            actual: "bb".into(),
        };
        assert!(format!("{mismatch}").contains("sha256 mismatch — expected aa, got bb"));
    }

    #[test]
    fn resolve_sandbox_user_reads_name_and_uid() {
        let env = HashMap::from([
            ("SANDBOX_USER".to_string(), "agent".to_string()),
            ("SANDBOX_UID".to_string(), "1000".to_string()),
        ]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.name, "agent");
        assert_eq!(user.uid, 1000);
    }

    #[test]
    fn resolve_sandbox_user_defaults_uid_when_unparseable_or_absent() {
        let env = HashMap::from([("SANDBOX_USER".to_string(), "agent".to_string())]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.uid, 65534);

        let env = HashMap::from([
            ("SANDBOX_USER".to_string(), "agent".to_string()),
            ("SANDBOX_UID".to_string(), "not-a-number".to_string()),
        ]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.uid, 65534);
    }

    #[test]
    fn resolve_sandbox_user_none_when_unset_or_empty() {
        assert!(resolve_sandbox_user(|_| None).is_none());
        let env = HashMap::from([("SANDBOX_USER".to_string(), String::new())]);
        assert!(resolve_sandbox_user(|k| env.get(k).cloned()).is_none());
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Mount {
            source: String,
            target: String,
            fstype: String,
            flags: MountFlags,
            data: Option<String>,
        },
        MoveMount {
            from: String,
            to: String,
        },
        Mkdir {
            path: String,
            mode: u32,
        },
        Symlink {
            target: String,
            linkpath: String,
        },
        MknodChar {
            path: String,
            mode: u32,
            major: u32,
            minor: u32,
        },
        Chroot(String),
        Chdir(String),
        PathExists(String),
        OpenRo(String),
        Fexecve(RawFd),
        Lchown {
            path: String,
            uid: u32,
            gid: u32,
        },
        Umount(String),
        WriteRootFile {
            path: String,
            contents: Vec<u8>,
            mode: u32,
        },
        WriteSysctl {
            path: String,
            value: String,
        },
        SeedVolume {
            seed_mount: String,
            image_target: String,
        },
        VerifyDigest {
            device: String,
            expected: String,
        },
    }

    type FailWhen = Box<dyn Fn(&Call) -> Option<std::io::ErrorKind>>;

    struct FakeSyscalls {
        calls: RefCell<Vec<Call>>,
        ptmx_exists: bool,
        fail_when: FailWhen,
        digest_result: RefCell<Option<Result<(), MountError>>>,
    }

    impl FakeSyscalls {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                ptmx_exists: true,
                fail_when: Box::new(|_| None),
                digest_result: RefCell::new(None),
            }
        }

        fn fail_when(mut self, f: impl Fn(&Call) -> Option<std::io::ErrorKind> + 'static) -> Self {
            self.fail_when = Box::new(f);
            self
        }

        fn ptmx_missing(mut self) -> Self {
            self.ptmx_exists = false;
            self
        }

        fn digest_err(self, err: MountError) -> Self {
            *self.digest_result.borrow_mut() = Some(Err(err));
            self
        }

        fn record(&self, call: Call) -> std::io::Result<()> {
            let outcome = (self.fail_when)(&call);
            self.calls.borrow_mut().push(call);
            match outcome {
                Some(kind) => Err(std::io::Error::new(kind, "scripted failure")),
                None => Ok(()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl Syscalls for FakeSyscalls {
        fn mount(
            &self,
            source: &CStr,
            target: &CStr,
            fstype: &CStr,
            flags: MountFlags,
            data: Option<&CStr>,
        ) -> std::io::Result<()> {
            self.record(Call::Mount {
                source: source.to_str().unwrap().to_string(),
                target: target.to_str().unwrap().to_string(),
                fstype: fstype.to_str().unwrap().to_string(),
                flags,
                data: data.map(|d| d.to_str().unwrap().to_string()),
            })
        }
        fn move_mount(&self, from: &CStr, to: &CStr) -> std::io::Result<()> {
            self.record(Call::MoveMount {
                from: from.to_str().unwrap().to_string(),
                to: to.to_str().unwrap().to_string(),
            })
        }
        fn mkdir(&self, path: &CStr, mode: u32) -> std::io::Result<()> {
            self.record(Call::Mkdir {
                path: path.to_str().unwrap().to_string(),
                mode,
            })
        }
        fn symlink(&self, target: &CStr, linkpath: &CStr) -> std::io::Result<()> {
            self.record(Call::Symlink {
                target: target.to_str().unwrap().to_string(),
                linkpath: linkpath.to_str().unwrap().to_string(),
            })
        }
        fn mknod_char(
            &self,
            path: &CStr,
            mode: u32,
            major: u32,
            minor: u32,
        ) -> std::io::Result<()> {
            self.record(Call::MknodChar {
                path: path.to_str().unwrap().to_string(),
                mode,
                major,
                minor,
            })
        }
        fn chroot(&self, path: &CStr) -> std::io::Result<()> {
            self.record(Call::Chroot(path.to_str().unwrap().to_string()))
        }
        fn chdir(&self, path: &CStr) -> std::io::Result<()> {
            self.record(Call::Chdir(path.to_str().unwrap().to_string()))
        }
        fn path_exists(&self, path: &CStr) -> bool {
            self.calls
                .borrow_mut()
                .push(Call::PathExists(path.to_str().unwrap().to_string()));
            self.ptmx_exists
        }
        fn open_ro(&self, path: &CStr) -> std::io::Result<RawFd> {
            self.record(Call::OpenRo(path.to_str().unwrap().to_string()))?;
            Ok(7)
        }
        fn fexecve(&self, fd: RawFd, _argv0: &CStr) -> std::io::Error {
            self.calls.borrow_mut().push(Call::Fexecve(fd));
            std::io::Error::from_raw_os_error(libc::ENOEXEC)
        }
        fn lchown(&self, path: &CStr, uid: u32, gid: u32) -> std::io::Result<()> {
            self.record(Call::Lchown {
                path: path.to_str().unwrap().to_string(),
                uid,
                gid,
            })
        }
        fn umount(&self, target: &CStr) -> std::io::Result<()> {
            self.record(Call::Umount(target.to_str().unwrap().to_string()))
        }
        fn write_root_file(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()> {
            self.record(Call::WriteRootFile {
                path: path.to_string(),
                contents: contents.to_vec(),
                mode,
            })
        }
        fn write_sysctl(&self, path: &str, value: &str) -> std::io::Result<()> {
            self.record(Call::WriteSysctl {
                path: path.to_string(),
                value: value.to_string(),
            })
        }
        fn seed_pristine_volume(
            &self,
            seed_mount: &str,
            image_target: &str,
        ) -> Result<(), MountError> {
            self.calls.borrow_mut().push(Call::SeedVolume {
                seed_mount: seed_mount.to_string(),
                image_target: image_target.to_string(),
            });
            Ok(())
        }
        fn verify_descriptor_digest(
            &self,
            device_path: &str,
            expected_hex: &str,
        ) -> Result<(), MountError> {
            self.calls.borrow_mut().push(Call::VerifyDigest {
                device: device_path.to_string(),
                expected: expected_hex.to_string(),
            });
            self.digest_result.borrow_mut().take().unwrap_or(Ok(()))
        }
    }

    fn full_params() -> CmdlineParams {
        CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
        }
    }

    const FULL_CMDLINE: &str = "console=hvc0 upper.dev=/dev/vda \
         composefs.descriptor.dev=/dev/vdb content.tag=lns-content \
         LENS_SANDBOX_TOKEN=deadbeefsecret LENS_SANDBOX_WS_URL=vsock://host:1024/v1/sandbox \
         AGENT_COMMAND_B64=ZWNobyBoaQ==";

    #[test]
    fn boot_sequence_records_every_step_in_order_then_fexecve() {
        let sys = FakeSyscalls::new();
        let params = full_params();
        let err =
            mount_composefs_and_exec_broker_inner(&params, None, "/newroot", FULL_CMDLINE, &sys)
                .unwrap_err();
        // The sequence always terminates in fexecve (only returns on error).
        assert!(matches!(err, MountError::Syscall { op, .. } if op.contains("fexecve")));
        let calls = sys.calls();
        assert_eq!(
            calls.first(),
            Some(&Call::Mount {
                source: "none".into(),
                target: SYS.into(),
                fstype: "sysfs".into(),
                flags: MountFlags::None,
                data: None,
            })
        );
        // erofs descriptor is mounted read-only; upper as ext4 rw.
        assert!(calls.contains(&Call::Mount {
            source: "/dev/vdb".into(),
            target: COMPOSEFS_META.into(),
            fstype: "erofs".into(),
            flags: MountFlags::ReadOnly,
            data: None,
        }));
        // overlay is mounted onto the provided newroot with the data-only-lower opts.
        assert!(calls.iter().any(|c| matches!(c, Call::Mount { target, fstype, data, .. }
            if target == "/newroot" && fstype == "overlay" && data.as_deref().unwrap().contains("::"))));
        // proc/sys/dev are moved under newroot; chroot happens after chdir(newroot).
        let move_root = calls
            .iter()
            .position(|c| matches!(c, Call::MoveMount { from, to } if from == "." && to == "/"))
            .unwrap();
        let chroot = calls
            .iter()
            .position(|c| matches!(c, Call::Chroot(p) if p == "."))
            .unwrap();
        assert!(move_root < chroot, "move_mount(.,/) precedes chroot(.)");
        assert!(matches!(calls.last(), Some(Call::Fexecve(7))));
    }

    #[test]
    fn boot_skips_ensure_ptmx_when_already_present() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            !sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Symlink { linkpath, .. } if linkpath == DEV_PTMX))
        );
    }

    #[test]
    fn boot_ensures_ptmx_via_symlink_when_missing() {
        let sys = FakeSyscalls::new().ptmx_missing();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Symlink { linkpath, .. } if linkpath == DEV_PTMX))
        );
        // symlink succeeded → mknod fallback not taken.
        assert!(
            !sys.calls()
                .iter()
                .any(|c| matches!(c, Call::MknodChar { .. }))
        );
    }

    #[test]
    fn boot_links_dev_fd_and_std_streams_into_proc_self_fd() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        for (target, link) in [
            ("/proc/self/fd", "/dev/fd"),
            ("/proc/self/fd/0", "/dev/stdin"),
            ("/proc/self/fd/1", "/dev/stdout"),
            ("/proc/self/fd/2", "/dev/stderr"),
        ] {
            assert!(
                calls.iter().any(|c| matches!(
                    c,
                    Call::Symlink { target: t, linkpath: l } if t == target && l == link
                )),
                "missing symlink {link} -> {target}"
            );
        }
    }

    #[test]
    fn dev_fd_links_tolerate_an_already_existing_link() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Symlink { linkpath, .. } if linkpath == "/dev/fd")
                .then_some(ErrorKind::AlreadyExists)
        });
        ensure_dev_fd_links(&sys).expect("an existing /dev/fd is benign, not fatal");
    }

    #[test]
    fn dev_fd_links_abort_boot_on_an_unexpected_symlink_error() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Symlink { linkpath, .. } if linkpath == "/dev/stdout")
                .then_some(ErrorKind::PermissionDenied)
        });
        let err = ensure_dev_fd_links(&sys).unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("/dev/stdout")),
            "got {err:?}"
        );
    }

    #[test]
    fn ensure_ptmx_falls_back_to_mknod_when_symlink_fails() {
        let sys = FakeSyscalls::new().ptmx_missing().fail_when(|c| {
            matches!(c, Call::Symlink { .. }).then_some(ErrorKind::PermissionDenied)
        });
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(sys.calls().iter().any(|c| matches!(
            c,
            Call::MknodChar {
                major: 5,
                minor: 2,
                ..
            }
        )));
    }

    #[test]
    fn ensure_ptmx_errors_when_both_symlink_and_mknod_fail() {
        let sys = FakeSyscalls::new().ptmx_missing().fail_when(|c| {
            matches!(c, Call::Symlink { .. } | Call::MknodChar { .. })
                .then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("symlink=") && msg.contains("mknod="),
            "combined error: {msg}"
        );
    }

    #[test]
    fn first_mount_failure_is_reported_with_op_string() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Mount { fstype, .. } if fstype == "sysfs")
                .then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("sysfs")),
            "got {err:?}"
        );
    }

    #[test]
    fn mkdir_already_exists_is_idempotent_and_boot_continues() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Mkdir { path, .. } if path == DEV_PTS)
                .then_some(ErrorKind::AlreadyExists)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("fexecve")),
            "got {err:?}"
        );
    }

    #[test]
    fn mkdir_hard_failure_is_reported_with_op_string() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Mkdir { path, .. } if path == DEV_PTS)
                .then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("mkdir(/dev/pts)")),
            "got {err:?}"
        );
    }

    #[test]
    fn move_mount_failure_is_reported_with_op_string() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::MoveMount { .. }).then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("mount --move")),
            "got {err:?}"
        );
    }

    #[test]
    fn chdir_failure_is_reported_with_op_string() {
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Chdir(_)).then_some(ErrorKind::PermissionDenied));
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("chdir(/newroot)")),
            "got {err:?}"
        );
    }

    #[test]
    fn chroot_failure_is_reported_with_op_string() {
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Chroot(_)).then_some(ErrorKind::PermissionDenied));
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("chroot(.)")),
            "got {err:?}"
        );
    }

    #[test]
    fn digest_mismatch_aborts_before_mounting_erofs() {
        let sys = FakeSyscalls::new().digest_err(MountError::DescriptorDigestMismatch {
            expected: "aa".into(),
            actual: "bb".into(),
        });
        let mut params = full_params();
        params.composefs_descriptor_sha256 = Some("aa".into());
        let err =
            mount_composefs_and_exec_broker_inner(&params, None, "/newroot", FULL_CMDLINE, &sys)
                .unwrap_err();
        assert!(matches!(err, MountError::DescriptorDigestMismatch { .. }));
        // erofs mount must NOT have happened after the digest check failed.
        assert!(
            !sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Mount { fstype, .. } if fstype == "erofs"))
        );
    }

    #[test]
    fn digest_verified_when_sha_present_then_proceeds() {
        let sys = FakeSyscalls::new();
        let mut params = full_params();
        params.composefs_descriptor_sha256 = Some("deadbeef".into());
        let _ =
            mount_composefs_and_exec_broker_inner(&params, None, "/newroot", FULL_CMDLINE, &sys);
        assert!(
            sys.calls().iter().any(
                |c| matches!(c, Call::VerifyDigest { expected, .. } if expected == "deadbeef")
            )
        );
    }

    #[test]
    fn open_broker_failure_aborts_before_pivot() {
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::OpenRo(_)).then_some(ErrorKind::NotFound));
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("open(")),
            "got {err:?}"
        );
        // The pivot (chroot) must not run once the broker fd couldn't be opened.
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Chroot(_))));
    }

    #[test]
    fn writes_sandbox_user_into_newroot_and_chowns_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "agent".into(),
            uid: 4242,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert!(passwd.contains("agent:x:4242:4242:"));
        let group = std::fs::read_to_string(format!("{newroot}/etc/group")).unwrap();
        assert!(group.contains("agent:x:4242:"));
        assert!(
            std::fs::metadata(format!("{newroot}/home/agent"))
                .unwrap()
                .is_dir()
        );
        assert!(
            sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Lchown { uid: 4242, .. }))
        );
    }

    #[test]
    fn write_sandbox_user_is_idempotent_and_appends_newline_to_seeded_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        std::fs::create_dir_all(format!("{newroot}/etc")).unwrap();
        // Seeded files lack a trailing newline → exercises the newline-insertion branch.
        std::fs::write(
            format!("{newroot}/etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh",
        )
        .unwrap();
        std::fs::write(format!("{newroot}/etc/group"), "root:x:0:").unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "agent".into(),
            uid: 1000,
        };
        write_sandbox_user_if_missing(newroot, &user, &sys);
        write_sandbox_user_if_missing(newroot, &user, &sys);
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert_eq!(
            passwd.matches("agent:x:1000").count(),
            1,
            "no duplicate user line"
        );
        assert!(
            passwd.starts_with("root:x:0:0:root:/root:/bin/sh\nagent:"),
            "newline inserted before appended user: {passwd:?}"
        );
        let group = std::fs::read_to_string(format!("{newroot}/etc/group")).unwrap();
        assert!(
            group.starts_with("root:x:0:\nagent:x:1000:"),
            "newline inserted before appended group: {group:?}"
        );
    }

    #[test]
    fn write_sandbox_user_reports_failed_lchown_without_panicking() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Lchown { .. }).then_some(ErrorKind::PermissionDenied));
        let user = SandboxUser {
            name: "agent".into(),
            uid: 1000,
        };
        write_sandbox_user_if_missing(newroot, &user, &sys);
        assert!(
            std::fs::metadata(format!("{newroot}/home/agent"))
                .unwrap()
                .is_dir()
        );
    }

    #[test]
    fn write_sandbox_user_skips_home_setup_on_nul_in_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "ag\0ent".into(),
            uid: 1000,
        };
        write_sandbox_user_if_missing(newroot, &user, &sys);
        // The interior NUL makes the home CString fail → home setup skipped, no lchown attempted.
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Lchown { .. })));
        // passwd is still (best-effort) written before the home step.
        assert!(std::fs::metadata(format!("{newroot}/etc/passwd")).is_ok());
    }

    fn volume(dev: &str, target: &str, read_only: bool) -> crate::cmdline::VolumeParam {
        crate::cmdline::VolumeParam {
            dev: dev.into(),
            target: target.into(),
            read_only,
        }
    }

    #[test]
    fn mount_volumes_seeds_each_device_once_then_mounts_every_target_under_newroot() {
        let sys = FakeSyscalls::new();
        let volumes = vec![
            volume("/dev/vdc", "/data", false),
            volume("/dev/vdc", "/cache", true),
        ];
        mount_volumes(&sys, &volumes, "/newroot").unwrap();
        let calls = sys.calls();

        let seeds: Vec<_> = calls
            .iter()
            .filter(|c| matches!(c, Call::SeedVolume { .. }))
            .collect();
        assert_eq!(
            seeds.len(),
            1,
            "shared device seeded exactly once: {calls:?}"
        );
        assert!(matches!(
            seeds[0],
            Call::SeedVolume { image_target, .. } if image_target == "/newroot/data"
        ));
        // Seeding mounts the device at the staging path and unmounts it afterward.
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Umount(t) if t == VOLUME_SEED_MOUNT))
        );
        // Both targets are mounted ext4 under newroot; the second is read-only.
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { target, fstype, flags, .. }
            if target == "/newroot/data" && fstype == "ext4" && *flags == MountFlags::None))
        );
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { target, flags, .. }
            if target == "/newroot/cache" && *flags == MountFlags::ReadOnly))
        );
    }

    #[test]
    fn mount_volumes_surfaces_a_failed_seed_unmount_with_its_op_string() {
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Umount(_)).then_some(ErrorKind::PermissionDenied));
        let err =
            mount_volumes(&sys, &[volume("/dev/vdc", "/data", false)], "/newroot").unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("umount(/mnt/vol-seed)")),
            "got {err:?}"
        );
    }

    #[test]
    fn boot_sequence_mounts_volumes_after_overlay_and_before_pivot() {
        let sys = FakeSyscalls::new();
        let mut params = full_params();
        params.volumes = vec![volume("/dev/vdc", "/data", false)];
        let _ =
            mount_composefs_and_exec_broker_inner(&params, None, "/newroot", FULL_CMDLINE, &sys);
        let calls = sys.calls();
        let overlay = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { fstype, .. } if fstype == "overlay"))
            .unwrap();
        let vol_mount = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { target, .. } if target == "/newroot/data"))
            .unwrap();
        let chroot = calls
            .iter()
            .position(|c| matches!(c, Call::Chroot(_)))
            .unwrap();
        assert!(overlay < vol_mount, "volume mounts after the overlay root");
        assert!(vol_mount < chroot, "volume mounts before the pivot/chroot");
    }

    #[test]
    fn boot_mounts_run_as_a_hardened_writable_tmpfs() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let run = sys
            .calls()
            .into_iter()
            .find_map(|c| match c {
                Call::Mount {
                    target,
                    fstype,
                    flags,
                    data,
                    ..
                } if target == "/newroot/run" && fstype == "tmpfs" => Some((flags, data)),
                _ => None,
            })
            .expect("/run is mounted as tmpfs under newroot");
        let (flags, data) = run;
        assert_eq!(flags, MountFlags::Tmpfs, "/run must be nosuid,nodev");
        let opts = data.expect("/run tmpfs carries options");
        assert!(opts.contains("mode=0755"), "/run must be 0755: {opts}");
        assert!(opts.contains("size="), "/run must be size-capped: {opts}");
    }

    #[test]
    fn boot_mounts_run_lock_as_a_sticky_noexec_tmpfs() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let lock = sys
            .calls()
            .into_iter()
            .find_map(|c| match c {
                Call::Mount {
                    target,
                    fstype,
                    flags,
                    data,
                    ..
                } if target == "/newroot/run/lock" && fstype == "tmpfs" => Some((flags, data)),
                _ => None,
            })
            .expect("/run/lock is mounted as tmpfs under newroot");
        let (flags, data) = lock;
        assert_eq!(
            flags,
            MountFlags::TmpfsNoExec,
            "/run/lock must be nosuid,nodev,noexec"
        );
        assert!(
            data.expect("/run/lock tmpfs carries options")
                .contains("mode=1777"),
            "/run/lock must be world-writable sticky 1777"
        );
    }

    #[test]
    fn boot_mounts_run_after_overlay_and_before_pivot_with_lock_nested() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        let overlay = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { fstype, .. } if fstype == "overlay"))
            .unwrap();
        let run = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { target, fstype, .. } if target == "/newroot/run" && fstype == "tmpfs"))
            .unwrap();
        let lock = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { target, .. } if target == "/newroot/run/lock"))
            .unwrap();
        let chroot = calls
            .iter()
            .position(|c| matches!(c, Call::Chroot(_)))
            .unwrap();
        assert!(
            overlay < run,
            "/run tmpfs mounts after the overlay root exists"
        );
        assert!(run < lock, "/run/lock mounts onto the fresh /run tmpfs");
        assert!(
            lock < chroot,
            "the writable /run is in place before the pivot"
        );
    }

    #[test]
    fn boot_chowns_run_to_the_sandbox_user_so_an_unprivileged_workload_can_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "agent".into(),
            uid: 4242,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let run_target = format!("{newroot}/run");
        assert!(
            sys.calls().iter().any(|c| matches!(
                c,
                Call::Lchown { path, uid: 4242, gid: 4242 } if path == &run_target
            )),
            "/run must be chowned to the sandbox user so it is actually writable"
        );
    }

    #[test]
    fn boot_leaves_run_root_owned_when_there_is_no_sandbox_user() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            !sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Lchown { path, .. } if path == "/newroot/run")),
            "a rootful workload keeps the standard root-owned 0755 /run"
        );
    }

    #[test]
    fn boot_aborts_when_run_chown_fails_rather_than_run_with_an_unwritable_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let run_target = format!("{newroot}/run");
        let fail_on = run_target.clone();
        let sys = FakeSyscalls::new().fail_when(move |c| {
            matches!(c, Call::Lchown { path, .. } if path == &fail_on)
                .then_some(std::io::ErrorKind::PermissionDenied)
        });
        let user = SandboxUser {
            name: "agent".into(),
            uid: 4242,
        };
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(matches!(err, MountError::Syscall { op, .. } if op.contains("lchown")));
    }

    #[test]
    fn boot_enables_unprivileged_low_port_binding_so_a_nonroot_workload_can_bind_port_80() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            sys.calls().iter().any(|c| matches!(
                c,
                Call::WriteSysctl { path, value }
                    if path == "/proc/sys/net/ipv4/ip_unprivileged_port_start" && value == "0"
            )),
            "unprivileged workloads must be allowed to bind ports below 1024"
        );
    }

    #[test]
    fn boot_writes_the_port_sysctl_while_proc_is_still_mounted_at_proc() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        let sysctl = calls
            .iter()
            .position(
                |c| matches!(c, Call::WriteSysctl { path, .. } if path == UNPRIV_PORT_START_SYSCTL),
            )
            .unwrap();
        let proc_move = calls
            .iter()
            .position(|c| matches!(c, Call::MoveMount { from, .. } if from == PROC))
            .unwrap();
        assert!(
            sysctl < proc_move,
            "the sysctl must be written before /proc is moved off its mountpoint"
        );
    }

    #[test]
    fn boot_aborts_when_the_low_port_sysctl_cannot_be_written() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::WriteSysctl { .. }).then_some(std::io::ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(err, MountError::Syscall { op, .. } if op.contains("ip_unprivileged_port_start"))
        );
    }

    #[test]
    fn boot_writes_a_0600_cmdline_mask_with_the_relay_token_stripped() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let write = sys
            .calls()
            .into_iter()
            .find_map(|c| match c {
                Call::WriteRootFile {
                    path,
                    contents,
                    mode,
                } if path == "/newroot/.lens/.cmdline" => Some((contents, mode)),
                _ => None,
            })
            .expect("masked cmdline file written under newroot");
        let (contents, mode) = write;
        assert_eq!(mode, 0o600, "mask file must be root-only 0600");
        let text = String::from_utf8(contents).unwrap();
        assert!(
            !text.contains("deadbeefsecret"),
            "the relay token value leaked into the mask file: {text}"
        );
        assert!(
            !text.contains("LENS_SANDBOX_"),
            "no internal var may survive into the mask file: {text}"
        );
        assert!(
            text.contains("upper.dev=/dev/vda") && text.contains("AGENT_COMMAND_B64="),
            "non-secret boot/workload tokens must be preserved: {text}"
        );
    }

    #[test]
    fn boot_binds_the_mask_over_proc_cmdline_after_proc_is_moved_into_newroot() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        let proc_move = calls
            .iter()
            .position(|c| matches!(c, Call::MoveMount { from, to } if from == PROC && to == "/newroot/proc"))
            .expect("proc is moved into newroot");
        let mask_bind = calls
            .iter()
            .position(|c| {
                matches!(c, Call::Mount { target, flags, .. }
                if target == "/newroot/proc/cmdline" && *flags == MountFlags::Bind)
            })
            .expect("masked cmdline is bind-mounted over /newroot/proc/cmdline");
        let chroot = calls
            .iter()
            .position(|c| matches!(c, Call::Chroot(_)))
            .expect("chroot happens");
        assert!(
            proc_move < mask_bind,
            "the mask must bind over proc/cmdline only after the real proc is moved in"
        );
        assert!(
            mask_bind < chroot,
            "the workload-visible cmdline must be masked before the pivot"
        );
    }

    #[test]
    fn boot_aborts_when_the_cmdline_mask_cannot_be_written() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::WriteRootFile { .. }).then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("masked cmdline")),
            "got {err:?}"
        );
        assert!(
            !sys.calls().iter().any(|c| matches!(c, Call::Chroot(_))),
            "a failed mask must abort the boot before the pivot, never exposing the token"
        );
    }

    #[test]
    fn boot_aborts_when_the_cmdline_mask_cannot_be_bind_mounted() {
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Mount { target, flags, .. }
                if target == "/newroot/proc/cmdline" && *flags == MountFlags::Bind)
            .then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("/newroot/proc/cmdline")),
            "got {err:?}"
        );
        assert!(
            !sys.calls().iter().any(|c| matches!(c, Call::Chroot(_))),
            "a failed mask bind must abort the boot before the pivot, never exposing the token"
        );
    }
}
