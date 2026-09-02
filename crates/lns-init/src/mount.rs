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
const DEV_NULL: &str = "/dev/null";
const DEV_PTS: &str = "/dev/pts";
const DEV_PTMX: &str = "/dev/ptmx";
const DEV_SHM: &str = "/dev/shm";
const DEV_SHM_TMPFS_OPTS: &str = "mode=1777";
const CONTENT: &str = "/content";
const COMPOSEFS_META: &str = "/composefs-meta";
const UPPER_MOUNTPOINT: &str = "/mnt/upper";
const NEWROOT: &str = "/newroot";
const INIT_BROKER_PATH: &str = "/init-broker";
const VOLUME_SEED_MOUNT: &str = "/mnt/vol-seed";
const VOLUME_MOUNT_OPTS: &str = "errors=remount-ro";
const PROC_CMDLINE: &str = "/proc/cmdline";
const CMDLINE_MASK_FILE: &str = "/.lens/.cmdline";
const DEV_FD_LINKS: &[(&str, &str)] = &[
    ("/proc/self/fd", "/dev/fd"),
    ("/proc/self/fd/0", "/dev/stdin"),
    ("/proc/self/fd/1", "/dev/stdout"),
    ("/proc/self/fd/2", "/dev/stderr"),
];
const EXPLODED_BIND_SHARES: &str = "/.lens/binds";
/// Where the boot reads the lower stack on its own, with no writable layer over it.
const LOWER_VIEW_MOUNT: &str = "/mnt/lower";
const RUN: &str = "/run";
const RUN_LOCK: &str = "/run/lock";
const TMP: &str = "/tmp";
const RUN_TMPFS_OPTS: &str = "mode=0755,size=64m";
const RUN_LOCK_TMPFS_OPTS: &str = "mode=1777,size=4m";
const TMP_TMPFS_OPTS: &str = "mode=1777";
const UNPRIV_PORT_START_SYSCTL: &str = "/proc/sys/net/ipv4/ip_unprivileged_port_start";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MountFlags {
    bind: bool,
    recursive: bool,
    read_only: bool,
    nosuid: bool,
    nodev: bool,
    noexec: bool,
}

impl MountFlags {
    fn none() -> Self {
        Self::default()
    }
    fn bind() -> Self {
        Self {
            bind: true,
            ..Self::default()
        }
    }
    /// A plain bind copies one mount; the masks an exclude leaves are submounts, and only a recursive bind carries them onto the entry it lands.
    fn bind_recursive() -> Self {
        Self {
            recursive: true,
            ..Self::bind()
        }
    }
    fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }
    fn nosuid(mut self) -> Self {
        self.nosuid = true;
        self
    }
    fn nodev(mut self) -> Self {
        self.nodev = true;
        self
    }
    fn noexec(mut self) -> Self {
        self.noexec = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Dir,
    File,
    Symlink(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareEntry {
    pub name: String,
    pub kind: EntryKind,
}

pub(crate) struct SandboxUser {
    pub name: String,
    pub uid: Option<u32>,
    pub group: Option<String>,
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
    fn is_dir(&self, path: &CStr) -> bool;
    fn read_dir(&self, path: &CStr) -> std::io::Result<Vec<ShareEntry>>;
    fn read_to_string(&self, path: &CStr) -> std::io::Result<Option<String>>;
    fn open_ro(&self, path: &CStr) -> std::io::Result<RawFd>;
    fn fexecve(&self, fd: RawFd, argv0: &CStr) -> std::io::Error;
    fn lchown(&self, path: &CStr, uid: u32, gid: u32) -> std::io::Result<()>;
    /// Removes the path when it is a symlink, without following it.
    fn unlink_if_symlink(&self, path: &CStr) -> std::io::Result<()>;
    fn umount(&self, target: &CStr) -> std::io::Result<()>;
    fn write_root_file(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()>;
    fn write_sysctl(&self, path: &str, value: &str) -> std::io::Result<()>;
    fn export_env(&self, key: &str, value: &str);
    fn seed_pristine_volume(
        &self,
        seed_mount: &str,
        image_target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), MountError>;
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
    IncompleteBind(usize),
    InteriorNul(&'static str),
    Syscall {
        op: String,
        err: std::io::Error,
    },
    DescriptorDigestMismatch {
        expected: String,
        actual: String,
    },
    DescriptorRead(std::io::Error),
    SeededPathUnderSymlink(String),
    ClaimWritesThroughBind {
        connector: String,
        claim: String,
        target: String,
    },
    ClaimUnderVolume {
        connector: String,
        claim: String,
        target: String,
    },
    UnresolvableUser(String),
    UnresolvableGroup(String),
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
            Self::IncompleteBind(idx) => write!(
                f,
                "bind.{idx} on the kernel cmdline is missing its tag/target or a declared \
                 dropped path (likely a truncated cmdline); refusing to boot rather than \
                 risk exposing a secret the operator chose to drop"
            ),
            Self::ClaimWritesThroughBind {
                connector,
                claim,
                target,
            } => write!(
                f,
                "connector {connector} writes {claim}, which lands inside the bind mounted at \
                 {target}; a granted file never reaches the host, so add that path to the \
                 bind's exclude or uninstall {connector}"
            ),
            Self::ClaimUnderVolume {
                connector,
                claim,
                target,
            } => write!(
                f,
                "connector {connector} writes {claim}, which lands inside the volume mounted at \
                 {target}; a volume takes no exclude, so there is nowhere guest-local for that \
                 file — move the volume off that path or uninstall {connector}"
            ),
            Self::SeededPathUnderSymlink(path) => write!(
                f,
                "the bind entry {path} on the way to a path a fileset or a granted connector writes is a symlink; \
                 refusing to boot rather than follow it and write somewhere the document \
                 did not name"
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
            Self::UnresolvableUser(name) => write!(
                f,
                "image USER {name:?} is absent from the image's /etc/passwd and carries no uid to seed; \
                 refusing to boot rather than silently run the workload as root"
            ),
            Self::UnresolvableGroup(group) => write!(
                f,
                "image USER group {group:?} is absent from the image's /etc/group and is not a numeric gid; \
                 refusing to boot rather than silently run the workload with the wrong group"
            ),
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

/// The same stack with no `upperdir`, which makes it read-only: what the host shipped, before any write a previous boot's workload left in the layer this run preserves.
fn lower_view_options() -> String {
    format!("lowerdir={COMPOSEFS_META}::{CONTENT},redirect_dir=on,metacopy=on")
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
    let uid = env_get("SANDBOX_UID").and_then(|s| s.parse().ok());
    let group = env_get("SANDBOX_GROUP").filter(|g| !g.is_empty());
    Some(SandboxUser { name, uid, group })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WorkloadIdentity {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) home: Option<String>,
}

impl WorkloadIdentity {
    pub(crate) fn ids(&self) -> (u32, u32) {
        (self.uid, self.gid)
    }
}

fn passwd_entry(newroot: &str, name: &str) -> Option<(u32, u32, Option<String>)> {
    let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        let uid = fields.nth(1)?.parse().ok()?;
        let gid = fields.next()?.parse().ok()?;
        let home = fields.nth(1).filter(|h| !h.is_empty()).map(str::to_string);
        Some((uid, gid, home))
    })
}

/// Resolve the identity the workload runs as, all from the one passwd line that supplies the uid — a home read from a sibling line would hand an exec a different identity's home. The uid comes from passwd or the seeded numeric id, the gid honors an explicit `USER` group and otherwise tracks the user's primary group, and a user or group that resolves to nothing is unresolvable — the caller refuses to boot rather than fall through to root.
pub(crate) fn resolve_workload_ids(
    newroot: &str,
    user: &SandboxUser,
    sys: &dyn Syscalls,
) -> Result<WorkloadIdentity, MountError> {
    let in_passwd = passwd_entry(newroot, &user.name);
    let uid = match &in_passwd {
        Some((uid, _, _)) => *uid,
        None => user
            .uid
            .ok_or_else(|| MountError::UnresolvableUser(user.name.clone()))?,
    };
    let gid = match &user.group {
        Some(spec) => resolve_group_gid(newroot, spec)
            .ok_or_else(|| MountError::UnresolvableGroup(spec.clone()))?,
        None => in_passwd.as_ref().map(|(_, gid, _)| *gid).unwrap_or(uid),
    };
    let home = match in_passwd {
        Some((_, _, home)) => home,
        None => {
            seed_sandbox_user(newroot, &user.name, uid, gid, sys);
            Some(format!("/home/{}", user.name))
        }
    };
    Ok(WorkloadIdentity { uid, gid, home })
}

/// `std::env::set_var` panics on interior NUL, and this is the one export whose bytes an image controls.
fn exportable_home(home: &str) -> bool {
    home.bytes().all(|b| b >= 0x20)
}

fn resolve_group_gid(newroot: &str, spec: &str) -> Option<u32> {
    spec.parse::<u32>()
        .ok()
        .or_else(|| group_gid(newroot, spec))
}

fn group_gid(newroot: &str, name: &str) -> Option<u32> {
    let group = std::fs::read_to_string(format!("{newroot}/etc/group")).ok()?;
    group.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == name).then_some(())?;
        fields.nth(1)?.parse().ok()
    })
}

const FILESET_OWNED_MANIFEST: &str = "/.lens/fileset-owned";

/// Where the service injects a run's declared tool trees; PID 1 stays dependency-free, so this is spelled here as well as in lns-service's tool cache.
const TOOLS_ROOT: &str = "/.lens/tools";

/// A tool writes inside its own prefix (the engine puts cargo's `CARGO_HOME` there) but every injected entry lands uid 0, so the resolved workload gets the directories — never the files — and its writes go to the overlay's upper layer and vanish at teardown.
fn chown_tool_dirs(sys: &dyn Syscalls, newroot: &str, run_ids: Option<(u32, u32)>) {
    let Some((uid, gid)) = run_ids else {
        return;
    };
    let root = format!("{newroot}{TOOLS_ROOT}");
    if !std::path::Path::new(&root).is_dir() {
        return;
    }
    let mut pending = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = pending.pop() {
        lchown_logged(sys, &dir.to_string_lossy(), uid, gid);
        // A directory the walk cannot read costs its subtree, not the rest of the trees.
        let Ok(listing) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in listing.flatten() {
            // Symlinks are not followed: a link's own target is already an entry of this tree, and following one could walk out of it.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
}

/// Transfer the fileset paths the definition marked `owner: workload` to the run-as ids; the host ships the list inside the runtime layer because only the guest can resolve a named image user.
fn chown_fileset_owned(sys: &dyn Syscalls, newroot: &str, run_ids: Option<(u32, u32)>) {
    let Some((uid, gid)) = run_ids else {
        return;
    };
    let Ok(listing) = std::fs::read_to_string(format!("{newroot}{FILESET_OWNED_MANIFEST}")) else {
        return;
    };
    for path in listing.lines().map(str::trim).filter(|p| !p.is_empty()) {
        if !path.starts_with('/') || path.split('/').any(|seg| seg == "..") {
            eprintln!(
                "lns-init: skipping fileset-owned entry {path:?} — not an absolute in-root path"
            );
            continue;
        }
        lchown_logged(sys, &format!("{newroot}{path}"), uid, gid);
    }
}

/// The chown manifest is applied a second time here because the paths it names inside a volume did not exist during the first pass.
fn land_writes_the_mounts_would_have_hidden(
    sys: &dyn Syscalls,
    newroot: &str,
    volumes: &[crate::cmdline::VolumeParam],
    split_binds: &[String],
    run_ids: Option<(u32, u32)>,
) -> Result<(), MountError> {
    let mut targets: Vec<String> = volumes.iter().map(|vol| vol.target.clone()).collect();
    targets.extend_from_slice(split_binds);
    crate::staged::land(newroot, &targets).map_err(|err| MountError::Syscall {
        op: format!(
            "landing the writes staged under {}",
            crate::staged::STAGED_ROOT
        ),
        err,
    })?;
    chown_fileset_owned(sys, newroot, run_ids);
    Ok(())
}

/// Runs on the assembled rootfs — after every mount and after the staged writes land — so image, volume, bind, and fileset all keep their own hosts file, and every session reads the one this boot leaves behind.
fn seed_loopback_names(newroot: &str) {
    if let Err(err) = crate::hosts::seed_if_absent(newroot) {
        eprintln!("lns-init: could not seed the guest hosts file: {err}");
    }
}

/// One place converts a path for `lchown` and logs what went wrong, so a path the syscall cannot take or a failure on one entry never abandons the rest of the walk.
fn lchown_logged(sys: &dyn Syscalls, path: &str, uid: u32, gid: u32) {
    match CString::new(path) {
        Ok(c_path) => {
            if let Err(err) = sys.lchown(&c_path, uid, gid) {
                eprintln!("lns-init: lchown({path:?}, {uid}, {gid}) failed: {err}");
            }
        }
        Err(_) => eprintln!("lns-init: skipping {path:?} — interior NUL"),
    }
}

fn seed_sandbox_user(newroot: &str, name: &str, uid: u32, gid: u32, sys: &dyn Syscalls) {
    let etc = format!("{newroot}/etc");
    let _ = std::fs::create_dir_all(&etc);

    prepend_user_line(&format!("{etc}/passwd"), name, uid, gid);
    prepend_group_line(&format!("{etc}/group"), name, gid);

    let home = format!("{newroot}/home/{name}");
    match CString::new(home.as_str()) {
        Ok(home_c) => {
            if std::fs::create_dir_all(&home).is_ok() {
                let _ = std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755));
                if let Err(err) = sys.lchown(&home_c, uid, gid) {
                    eprintln!("lns-init: lchown({home:?}, {uid}, {gid}) failed: {err}");
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

// Prepended, not appended: first-match uid→name resolution (whoami, id) must surface this identity over an image alias for the same uid (alpine's nobody:65534).
fn prepend_user_line(passwd_path: &str, user: &str, uid: u32, gid: u32) {
    let existing = std::fs::read_to_string(passwd_path).unwrap_or_default();
    let prefix = format!("{user}:");
    if existing.lines().any(|l| l.starts_with(&prefix)) {
        return;
    }
    let updated = format!("{user}:x:{uid}:{gid}:LNS sandbox user:/home/{user}:/bin/sh\n{existing}");
    let _ = std::fs::write(passwd_path, updated);
}

fn prepend_group_line(group_path: &str, group: &str, gid: u32) {
    let existing = std::fs::read_to_string(group_path).unwrap_or_default();
    let prefix = format!("{group}:");
    if existing.lines().any(|l| l.starts_with(&prefix)) {
        return;
    }
    let updated = format!("{group}:x:{gid}:\n{existing}");
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
    if let Some(&idx) = p.incomplete_binds.first() {
        return Err(MountError::IncompleteBind(idx));
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
    debug_assert!(
        !flags.bind || !(flags.nosuid || flags.nodev || flags.noexec),
        "a bind mount ignores nosuid/nodev/noexec in the initial mount() call; apply them with a separate MS_REMOUNT"
    );
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

fn do_mount_pseudo(
    sys: &dyn Syscalls,
    target: &str,
    fstype: &str,
    flags: MountFlags,
) -> Result<(), MountError> {
    do_mount(sys, "none", target, fstype, flags, None)
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

fn do_mkdir(sys: &dyn Syscalls, path: &str, mode: u32) -> Result<bool, MountError> {
    let c = cstring(path, "mkdir-path")?;
    match sys.mkdir(&c, mode) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(MountError::Syscall {
            op: format!("mkdir({path})"),
            err,
        }),
    }
}

fn do_mkdir_p(sys: &dyn Syscalls, path: &str, mode: u32) -> Result<(), MountError> {
    made_by_mkdir_p(sys, path, mode).map(|_| ())
}

/// Every component this call brought into being, so a caller can hand them to whoever will use them; one the image already shipped is not this boot's to give away.
fn made_by_mkdir_p(sys: &dyn Syscalls, path: &str, mode: u32) -> Result<Vec<String>, MountError> {
    let mut acc = String::with_capacity(path.len());
    let mut made = Vec::new();
    for component in path.split('/').filter(|c| !c.is_empty()) {
        acc.push('/');
        acc.push_str(component);
        if do_mkdir(sys, &acc, mode)? {
            made.push(acc.clone());
        }
    }
    Ok(made)
}

fn allow_unprivileged_low_ports(sys: &dyn Syscalls) -> Result<(), MountError> {
    sys.write_sysctl(UNPRIV_PORT_START_SYSCTL, "0")
        .map_err(|err| MountError::Syscall {
            op: format!("write {UNPRIV_PORT_START_SYSCTL}"),
            err,
        })
}

fn do_lchown(sys: &dyn Syscalls, path: &str, uid: u32, gid: u32) -> Result<(), MountError> {
    let c = cstring(path, "lchown-path")?;
    sys.lchown(&c, uid, gid).map_err(|err| MountError::Syscall {
        op: format!("lchown({path}, {uid}:{gid})"),
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

pub const VOLUME_TARGETS_ENV: &str = "LENS_VOLUME_TARGETS";

fn mount_volumes(
    sys: &dyn Syscalls,
    volumes: &[crate::cmdline::VolumeParam],
    newroot: &str,
    run_ids: Option<(u32, u32)>,
) -> Result<(), MountError> {
    let mut seeded: Vec<&str> = Vec::new();
    let mut mounted: Vec<&str> = Vec::new();
    for vol in volumes {
        let target = format!("{newroot}{}", vol.target);
        if !seeded.contains(&vol.dev.as_str()) {
            seed_volume_if_pristine(sys, &vol.dev, &target, run_ids)?;
            seeded.push(&vol.dev);
        }
        do_mkdir_p(sys, &target, 0o755)?;
        let flags = match vol.read_only {
            true => MountFlags::read_only().nosuid().nodev(),
            false => MountFlags::none().nosuid().nodev(),
        };
        do_mount(
            sys,
            &vol.dev,
            &target,
            "ext4",
            flags,
            Some(VOLUME_MOUNT_OPTS),
        )?;
        if !vol.read_only {
            reconcile_volume_owner(sys, &target, run_ids);
        }
        mounted.push(&vol.target);
    }
    // Only lns-init knows which guest paths are volumes, and the broker has to release them before poweroff.
    if !mounted.is_empty() {
        sys.export_env(VOLUME_TARGETS_ENV, &mounted.join(":"));
    }
    Ok(())
}

/// Runs after the mount, so it lands on the volume's own root inode rather than the ephemeral directory the mount covers.
fn reconcile_volume_owner(sys: &dyn Syscalls, target: &str, run_ids: Option<(u32, u32)>) {
    use std::os::unix::fs::MetadataExt;
    let Some((uid, gid)) = run_ids else {
        return;
    };
    // Root writes any path already, and rewriting here would clobber the ownership an image seeded.
    if uid == 0 {
        return;
    }
    let Ok(md) = std::fs::metadata(target) else {
        return;
    };
    if (md.uid(), md.gid()) == (uid, gid) {
        return;
    }
    lchown_logged(sys, target, uid, gid);
}

/// A file the host shipped in the runtime layer, read from the lower stack alone, because the writable layer a run preserves is one a previous boot's workload could have deleted or rewritten its copy in.
fn read_from_the_lower_stack(
    sys: &dyn Syscalls,
    guest_path: &str,
) -> Result<Option<String>, MountError> {
    let path = format!("{LOWER_VIEW_MOUNT}{guest_path}");
    let path_c = cstring(&path, "lower-view-path")?;
    do_mkdir(sys, LOWER_VIEW_MOUNT, 0o700)?;
    do_mount(
        sys,
        "overlay",
        LOWER_VIEW_MOUNT,
        "overlay",
        MountFlags::read_only().nosuid().nodev().noexec(),
        Some(&lower_view_options()),
    )?;
    let body = sys.read_to_string(&path_c);
    do_umount(sys, LOWER_VIEW_MOUNT)?;
    body.map_err(|err| MountError::Syscall {
        op: format!("read {path}"),
        err,
    })
}

/// A landing belongs to the boot that makes it, so a symlink standing at one — a write a previous boot's workload left in the preserved writable layer — is unlinked rather than followed: following it lets the workload choose where root writes and mounts.
fn clear_a_symlink_standing_at(sys: &dyn Syscalls, landing: &str) -> Result<(), MountError> {
    let landing_c = cstring(landing, "landing-path")?;
    sys.unlink_if_symlink(&landing_c)
        .map_err(|err| MountError::Syscall {
            op: format!("unlink the symlink standing at {landing}"),
            err,
        })
}

/// One connector's write, resolved against this guest's home.
#[derive(Debug)]
struct Claim {
    connector: String,
    path: String,
}

/// A run whose user has no home takes no method that writes a file, so it makes room for none (§3.1.11).
fn connector_claims(sys: &dyn Syscalls, home: Option<&str>) -> Result<Vec<Claim>, MountError> {
    let Some(home) = home else {
        return Ok(Vec::new());
    };
    let Some(body) = read_from_the_lower_stack(sys, lns_placement::CONNECTOR_WRITES_MANIFEST)?
    else {
        return Ok(Vec::new());
    };
    Ok(body
        .lines()
        .filter_map(|line| claim_of(line, home))
        .collect())
}

/// A line the producer did not write is a claim of nothing, and re-anchoring it under the home would name a path no grant writes.
fn claim_of(line: &str, home: &str) -> Option<Claim> {
    let (connector, path) = line.trim().split_once('\t')?;
    let rest = path.strip_prefix("~/")?;
    Some(Claim {
        connector: connector.to_string(),
        path: format!("{}/{rest}", home.trim_end_matches('/')),
    })
}

/// A volume takes no `exclude`, so a claim landing under one has nowhere guest-local to go (§3.1.11).
fn refuse_a_claim_a_volume_would_take(
    volumes: &[crate::cmdline::VolumeParam],
    claims: &[Claim],
) -> Result<(), MountError> {
    for volume in volumes {
        for claim in claims {
            if lns_placement::place(&volume.target, &[] as &[&str], &claim.path)
                != lns_placement::Placement::Free
            {
                return Err(MountError::ClaimUnderVolume {
                    connector: claim.connector.clone(),
                    claim: claim.path.clone(),
                    target: volume.target.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Every exclude of this bind a claim needs left unmounted, and a refusal for a claim no exclude covers (§3.1.11).
fn seeds_the_claims_need(
    bind: &crate::cmdline::BindParam,
    claims: &[Claim],
) -> Result<Vec<String>, MountError> {
    let mut seeds = Vec::new();
    for claim in claims {
        match lns_placement::place(&bind.target, &bind.excluded_paths, &claim.path) {
            lns_placement::Placement::Free => continue,
            lns_placement::Placement::WritesThrough => {
                return Err(MountError::ClaimWritesThroughBind {
                    connector: claim.connector.clone(),
                    claim: claim.path.clone(),
                    target: bind.target.clone(),
                });
            }
            lns_placement::Placement::Masked { .. } => {
                for entry in &bind.excluded_paths {
                    if lns_placement::mask_covers(&bind.target, entry, &claim.path)
                        && !seeds.contains(entry)
                    {
                        seeds.push(entry.clone());
                    }
                }
            }
        }
    }
    Ok(seeds)
}

/// The targets of the binds this boot split, whose masks are guest-local directories a staged write lands in.
fn mount_binds(
    sys: &dyn Syscalls,
    binds: &[crate::cmdline::BindParam],
    volumes: &[crate::cmdline::VolumeParam],
    newroot: &str,
    run_ids: Option<(u32, u32)>,
    home: Option<&str>,
) -> Result<Vec<String>, MountError> {
    let nothing_can_bury_a_claim = binds.is_empty() && volumes.is_empty();
    let claims = match nothing_can_bury_a_claim {
        true => Vec::new(),
        false => connector_claims(sys, home)?,
    };
    refuse_a_claim_a_volume_would_take(volumes, &claims)?;
    let mut split = Vec::new();
    for (index, bind) in binds.iter().enumerate() {
        let mut seeds = seeds_the_claims_need(bind, &claims)?;
        for seeded in &bind.seeded_paths {
            if !seeds.contains(seeded) {
                seeds.push(seeded.clone());
            }
        }
        let target = format!("{newroot}{}", bind.target);
        // A whole bind covers whatever stands at its target, so only a split one needs the target to be the boot's own directory.
        if !seeds.is_empty() {
            clear_a_symlink_standing_at(sys, &target)?;
        }
        let made = made_by_mkdir_p(sys, &target, 0o755)?;
        hand_new_home_dirs_to_workload(sys, newroot, home, run_ids, &made);
        let flags = match bind.read_only {
            true => MountFlags::read_only().nosuid().nodev(),
            false => MountFlags::none().nosuid().nodev(),
        };
        if seeds.is_empty() {
            do_mount(sys, &bind.tag, &target, "virtiofs", flags, None)?;
            mask_each_dropped_path(sys, &target, &bind.dropped_paths)?;
            continue;
        }
        explode_bind(sys, bind, index, newroot, &target, flags, &seeds)?;
        split.push(bind.target.clone());
    }
    Ok(split)
}

fn mask_each_dropped_path(
    sys: &dyn Syscalls,
    root: &str,
    dropped_paths: &[String],
) -> Result<(), MountError> {
    for dropped in dropped_paths {
        mask_dropped_path(sys, &format!("{root}/{dropped}"))?;
    }
    Ok(())
}

/// A fileset writes into a path this bind excludes, and that write must not reach the share, so the share is mounted out of the way and its entries are bound in one at a time — leaving the excluded path's parents as guest-local directories the write can land in (§3.1.11).
fn explode_bind(
    sys: &dyn Syscalls,
    bind: &crate::cmdline::BindParam,
    index: usize,
    newroot: &str,
    target: &str,
    flags: MountFlags,
    seeds: &[String],
) -> Result<(), MountError> {
    let share = format!("{newroot}{EXPLODED_BIND_SHARES}/{index}");
    made_by_mkdir_p(sys, &share, 0o700)?;
    do_mount(sys, &bind.tag, &share, "virtiofs", flags, None)?;
    mask_each_dropped_path(sys, &share, &bind.dropped_paths)?;
    let spine: Vec<Vec<&str>> = seeds.iter().map(|path| path_segments(path)).collect();
    bind_the_entries_beside(sys, &share, target, &spine)
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn bind_the_entries_beside(
    sys: &dyn Syscalls,
    from: &str,
    onto: &str,
    spine: &[Vec<&str>],
) -> Result<(), MountError> {
    let from_c = cstring(from, "read-dir-path")?;
    let entries = sys.read_dir(&from_c).map_err(|err| MountError::Syscall {
        op: format!("read_dir({from})"),
        err,
    })?;
    for entry in entries {
        let deeper: Vec<Vec<&str>> = spine
            .iter()
            .filter(|path| path.first() == Some(&entry.name.as_str()))
            .map(|path| path[1..].to_vec())
            .collect();
        let source = format!("{from}/{}", entry.name);
        let landing = format!("{onto}/{}", entry.name);
        clear_a_symlink_standing_at(sys, &landing)?;
        if deeper.is_empty() {
            place_one_entry(sys, &entry, &source, &landing)?;
        } else if !deeper.iter().any(Vec::is_empty) {
            if let EntryKind::Symlink(_) = entry.kind {
                return Err(MountError::SeededPathUnderSymlink(source));
            }
            made_by_mkdir_p(sys, &landing, 0o755)?;
            bind_the_entries_beside(sys, &source, &landing, &deeper)?;
        }
    }
    Ok(())
}

/// A whole bind shows a symlink as a symlink, so the split recreates one rather than binding what it resolves to in this namespace; a bind clones the flags of the mount it copies, so the share's own nosuid/nodev/read-only reach every entry without a remount.
fn place_one_entry(
    sys: &dyn Syscalls,
    entry: &ShareEntry,
    source: &str,
    landing: &str,
) -> Result<(), MountError> {
    match &entry.kind {
        EntryKind::Symlink(target) => {
            let target_c = cstring(target, "symlink-target")?;
            let landing_c = cstring(landing, "symlink-path")?;
            sys.symlink(&target_c, &landing_c)
                .map_err(|err| MountError::Syscall {
                    op: format!("symlink({target} -> {landing})"),
                    err,
                })
        }
        EntryKind::Dir => {
            do_mkdir(sys, landing, 0o755)?;
            do_mount(
                sys,
                source,
                landing,
                "none",
                MountFlags::bind_recursive(),
                None,
            )
        }
        EntryKind::File => {
            sys.write_root_file(landing, b"", 0o644)
                .map_err(|err| MountError::Syscall {
                    op: format!("create bind mountpoint {landing}"),
                    err,
                })?;
            do_mount(
                sys,
                source,
                landing,
                "none",
                MountFlags::bind_recursive(),
                None,
            )
        }
    }
}

/// A mount point this boot created at or under the home belongs to the workload, or it can traverse the directory and write nothing beside what the mount put there.
fn hand_new_home_dirs_to_workload(
    sys: &dyn Syscalls,
    newroot: &str,
    home: Option<&str>,
    run_ids: Option<(u32, u32)>,
    made: &[String],
) {
    let (Some((uid, gid)), Some(home)) = (run_ids, home) else {
        return;
    };
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return;
    }
    let at_home = format!("{newroot}{home}");
    let under_home = format!("{at_home}/");
    for path in made
        .iter()
        .filter(|p| **p == at_home || p.starts_with(&under_home))
    {
        lchown_logged(sys, path, uid, gid);
    }
}

/// Hides a dropped secret from the workload without deleting it from the live share: an empty tmpfs over a directory, a `/dev/null` bind over a file.
fn mask_dropped_path(sys: &dyn Syscalls, path: &str) -> Result<(), MountError> {
    let path_c = cstring(path, "mask-path")?;
    if sys.is_dir(&path_c) {
        do_mount(
            sys,
            "tmpfs",
            path,
            "tmpfs",
            MountFlags::none().nosuid().nodev().noexec(),
            Some("mode=0000,size=4k"),
        )
    } else {
        do_mount(sys, DEV_NULL, path, "none", MountFlags::bind(), None)
    }
}

fn mount_run_tmpfs(
    sys: &dyn Syscalls,
    newroot: &str,
    run_ids: Option<(u32, u32)>,
) -> Result<(), MountError> {
    let run = format!("{newroot}{RUN}");
    do_mkdir(sys, &run, 0o755)?;
    do_mount(
        sys,
        "tmpfs",
        &run,
        "tmpfs",
        MountFlags::none().nosuid().nodev(),
        Some(RUN_TMPFS_OPTS),
    )?;
    if let Some((uid, gid)) = run_ids {
        do_lchown(sys, &run, uid, gid)?;
    }
    let lock = format!("{newroot}{RUN_LOCK}");
    do_mkdir(sys, &lock, 0o755)?;
    do_mount(
        sys,
        "tmpfs",
        &lock,
        "tmpfs",
        MountFlags::none().nosuid().nodev().noexec(),
        Some(RUN_LOCK_TMPFS_OPTS),
    )?;
    // /tmp is boot-scoped by decision: a restarted run finds it empty, so scratch never leaks into the preserved writable layer.
    let tmp = format!("{newroot}{TMP}");
    do_mkdir(sys, &tmp, 0o1777)?;
    do_mount(
        sys,
        "tmpfs",
        &tmp,
        "tmpfs",
        MountFlags::none().nosuid().nodev(),
        Some(TMP_TMPFS_OPTS),
    )
}

fn seed_volume_if_pristine(
    sys: &dyn Syscalls,
    dev: &str,
    image_target: &str,
    run_ids: Option<(u32, u32)>,
) -> Result<(), MountError> {
    do_mkdir(sys, VOLUME_SEED_MOUNT, 0o755).ok();
    do_mount(
        sys,
        dev,
        VOLUME_SEED_MOUNT,
        "ext4",
        MountFlags::none().nosuid().nodev(),
        Some(VOLUME_MOUNT_OPTS),
    )?;
    let (uid, gid) = run_ids.unwrap_or((0, 0));
    let result = sys.seed_pristine_volume(VOLUME_SEED_MOUNT, image_target, uid, gid);
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
    do_mount(sys, &mask_file, &target, "none", MountFlags::bind(), None)
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

/// No `size=`: the kernel's tmpfs default is half of guest RAM, which the run already bounds — Docker's 64M cap is the footgun `--disable-dev-shm-usage` exists for.
fn mount_dev_shm(sys: &dyn Syscalls) -> Result<(), MountError> {
    do_mkdir(sys, DEV_SHM, 0o1777)?;
    do_mount(
        sys,
        "tmpfs",
        DEV_SHM,
        "tmpfs",
        MountFlags::none().nosuid().nodev().noexec(),
        Some(DEV_SHM_TMPFS_OPTS),
    )
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
    do_mount_pseudo(
        sys,
        SYS,
        "sysfs",
        MountFlags::none().nosuid().nodev().noexec(),
    )?;
    do_mount_pseudo(sys, DEV, "devtmpfs", MountFlags::none().nosuid().noexec())?;

    do_mkdir(sys, DEV_PTS, 0o755)?;
    do_mount(
        sys,
        "devpts",
        DEV_PTS,
        "devpts",
        MountFlags::none().nosuid().noexec(),
        Some("gid=5,mode=620,ptmxmode=666"),
    )?;
    let ptmx_c = cstring(DEV_PTMX, "ptmx-path")?;
    if !sys.path_exists(&ptmx_c) {
        ensure_dev_ptmx(sys)?;
    }
    ensure_dev_fd_links(sys)?;

    // Shared memory is a convenience, not a boot requirement, so a kernel without it degrades instead of refusing the run.
    if let Err(err) = mount_dev_shm(sys) {
        eprintln!("lns-init: {DEV_SHM} unavailable, POSIX shared memory will fail: {err}");
    }

    let content_tag = params.content_tag.as_deref().unwrap();
    do_mount(
        sys,
        content_tag,
        CONTENT,
        "virtiofs",
        MountFlags::read_only().nosuid().nodev(),
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
        MountFlags::read_only().nosuid().nodev().noexec(),
        None,
    )?;

    let upper_dev = params.upper_dev.as_deref().unwrap();
    do_mount(
        sys,
        upper_dev,
        UPPER_MOUNTPOINT,
        "ext4",
        MountFlags::none().nosuid().nodev(),
        Some(VOLUME_MOUNT_OPTS),
    )?;

    let upper_upper = format!("{UPPER_MOUNTPOINT}/upper");
    let upper_work = format!("{UPPER_MOUNTPOINT}/work");
    do_mkdir(sys, &upper_upper, 0o755)?;
    do_mkdir(sys, &upper_work, 0o755)?;

    let opts = overlay_options();
    // The workload rootfs stays permissive: images legitimately ship and exec their own suid binaries.
    do_mount(
        sys,
        "overlay",
        newroot,
        "overlay",
        MountFlags::none(),
        Some(&opts),
    )?;

    let identity = match sandbox_user {
        Some(user) => Some(resolve_workload_ids(newroot, user, sys)?),
        None => None,
    };
    let run_ids = identity.as_ref().map(WorkloadIdentity::ids);

    if let Some(identity) = &identity {
        sys.export_env("LENS_RUN_UID", &identity.uid.to_string());
        sys.export_env("LENS_RUN_GID", &identity.gid.to_string());
        if let Some(home) = identity.home.as_deref().filter(|h| exportable_home(h)) {
            sys.export_env("LENS_RUN_HOME", home);
        }
    }

    chown_fileset_owned(sys, newroot, run_ids);

    chown_tool_dirs(sys, newroot, run_ids);

    mount_volumes(sys, &params.volumes, newroot, run_ids)?;

    let home = identity.as_ref().and_then(|i| i.home.as_deref());
    let split_binds = mount_binds(sys, &params.binds, &params.volumes, newroot, run_ids, home)?;

    land_writes_the_mounts_would_have_hidden(sys, newroot, &params.volumes, &split_binds, run_ids)?;

    seed_loopback_names(newroot);

    mount_run_tmpfs(sys, newroot, run_ids)?;

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
    use std::collections::{BTreeMap, HashMap};

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
            binds: vec![],
            incomplete_binds: vec![],
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
            binds: vec![],
            incomplete_binds: vec![],
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
            binds: vec![],
            incomplete_binds: vec![],
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
            binds: vec![],
            incomplete_binds: vec![],
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
            binds: vec![],
            incomplete_binds: vec![],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("volume.3"), "{err}");
    }

    #[test]
    fn validate_cmdline_refuses_to_boot_when_a_bind_drop_list_was_truncated() {
        let p = CmdlineParams {
            upper_dev: Some("/dev/vda".into()),
            composefs_descriptor_dev: Some("/dev/vdb".into()),
            content_tag: Some("lns-content".into()),
            composefs_descriptor_sha256: None,
            volumes: vec![],
            incomplete_volumes: vec![],
            binds: vec![],
            incomplete_binds: vec![1],
        };
        let err = validate_cmdline(&p).unwrap_err();
        assert!(format!("{err}").contains("bind.1"), "{err}");
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
        let incomplete_bind = MountError::IncompleteBind(2);
        assert!(format!("{incomplete_bind}").contains("bind.2"));
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
        let unresolvable = MountError::UnresolvableUser("www-data".into());
        assert!(format!("{unresolvable}").contains("refusing to boot"));
        let bad_group = MountError::UnresolvableGroup("staff".into());
        let rendered = format!("{bad_group}");
        assert!(rendered.contains("staff") && rendered.contains("refusing to boot"));
    }

    #[test]
    fn resolve_sandbox_user_reads_name_uid_and_group() {
        let env = HashMap::from([
            ("SANDBOX_USER".to_string(), "agent".to_string()),
            ("SANDBOX_UID".to_string(), "1000".to_string()),
            ("SANDBOX_GROUP".to_string(), "staff".to_string()),
        ]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.name, "agent");
        assert_eq!(user.uid, Some(1000));
        assert_eq!(user.group.as_deref(), Some("staff"));
    }

    #[test]
    fn resolve_sandbox_user_leaves_uid_and_group_unknown_when_unparseable_or_absent() {
        let env = HashMap::from([("SANDBOX_USER".to_string(), "agent".to_string())]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.uid, None);
        assert_eq!(user.group, None);

        let env = HashMap::from([
            ("SANDBOX_USER".to_string(), "agent".to_string()),
            ("SANDBOX_UID".to_string(), "not-a-number".to_string()),
            ("SANDBOX_GROUP".to_string(), String::new()),
        ]);
        let user = resolve_sandbox_user(|k| env.get(k).cloned()).expect("user");
        assert_eq!(user.uid, None);
        assert_eq!(
            user.group, None,
            "an empty SANDBOX_GROUP is treated as unset"
        );
    }

    #[test]
    fn resolve_sandbox_user_none_when_unset_or_empty() {
        assert!(resolve_sandbox_user(|_| None).is_none());
        let env = HashMap::from([("SANDBOX_USER".to_string(), String::new())]);
        assert!(resolve_sandbox_user(|k| env.get(k).cloned()).is_none());
    }

    fn newroot_with_passwd(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        std::fs::write(dir.path().join("etc/passwd"), contents).unwrap();
        dir
    }

    #[test]
    fn passwd_lookup_matches_by_exact_name_and_skips_malformed_lines() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             trunc:x\n\
             baduid:x:nope:5:::\n\
             badgid:x:1000:nope:::\n\
             nogid:x:1000\n\
             good:x:1000:2000:::\n",
        );
        let newroot = dir.path().to_str().unwrap();
        assert_eq!(passwd_entry(newroot, "good"), Some((1000, 2000, None)));
        assert_eq!(
            passwd_entry(newroot, "root"),
            Some((0, 0, Some("/root".to_string())))
        );
        assert_eq!(passwd_entry(newroot, "absent"), None);
        assert_eq!(passwd_entry(newroot, "trunc"), None);
        assert_eq!(passwd_entry(newroot, "baduid"), None);
        assert_eq!(passwd_entry(newroot, "badgid"), None);
        assert_eq!(passwd_entry(newroot, "nogid"), None);
    }

    #[test]
    fn passwd_lookup_is_none_when_the_image_has_no_passwd_file() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(passwd_entry(dir.path().to_str().unwrap(), "anyone"), None);
    }

    #[test]
    fn workload_ids_take_an_image_users_uid_gid_from_passwd_without_reseeding() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             appuser:x:1000:2000:app:/home/appuser:/bin/sh\n",
        );
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "appuser".into(),
            uid: None,
            group: None,
        };
        assert_eq!(
            resolve_workload_ids(newroot, &user, &sys).unwrap().ids(),
            (1000, 2000)
        );
        assert!(
            !sys.calls().iter().any(|c| matches!(c, Call::Lchown { .. })),
            "an image-defined user must not be re-chowned"
        );
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert_eq!(
            passwd.matches("appuser:x:").count(),
            1,
            "image-defined user must not be duplicated"
        );
    }

    #[test]
    fn workload_ids_seed_an_absent_user_at_its_requested_uid() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "sandbox".into(),
            uid: Some(65534),
            group: None,
        };
        assert_eq!(
            resolve_workload_ids(newroot, &user, &sys).unwrap().ids(),
            (65534, 65534)
        );
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert!(passwd.contains("sandbox:x:65534:65534:LNS sandbox user:"));
        assert!(sys.calls().iter().any(|c| matches!(
            c,
            Call::Lchown {
                uid: 65534,
                gid: 65534,
                ..
            }
        )));
    }

    #[test]
    fn workload_ids_seed_the_sandbox_user_ahead_of_an_image_alias_for_the_same_uid() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             nobody:x:65534:65534:nobody:/:/sbin/nologin\n",
        );
        std::fs::write(dir.path().join("etc/group"), "root:x:0:\nnobody:x:65534:\n").unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "sandbox".into(),
            uid: Some(65534),
            group: None,
        };
        assert_eq!(
            resolve_workload_ids(newroot, &user, &sys).unwrap().ids(),
            (65534, 65534)
        );
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert!(
            passwd.find("sandbox:x:65534:").unwrap() < passwd.find("nobody:x:65534:").unwrap(),
            "first-match uid resolution (whoami, id) must surface the sandbox identity, not the image's alias: {passwd}"
        );
        let group = std::fs::read_to_string(format!("{newroot}/etc/group")).unwrap();
        assert!(
            group.find("sandbox:x:65534:").unwrap() < group.find("nobody:x:65534:").unwrap(),
            "group name resolution must also prefer the sandbox identity: {group}"
        );
    }

    #[test]
    fn workload_ids_reject_a_named_user_with_unknown_uid_without_seeding_root() {
        let dir = newroot_with_passwd("root:x:0:0:root:/root:/bin/sh\n");
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "www-data".into(),
            uid: None,
            group: None,
        };
        assert!(matches!(
            resolve_workload_ids(newroot, &user, &sys),
            Err(MountError::UnresolvableUser(name)) if name == "www-data"
        ));
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert!(
            !passwd.contains("www-data"),
            "an unresolvable named user must not be seeded as uid 0"
        );
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Lchown { .. })));
    }

    #[test]
    fn workload_ids_honor_a_numeric_group_against_a_seeded_user() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "1001".into(),
            uid: Some(1001),
            group: Some("0".into()),
        };
        assert_eq!(
            resolve_workload_ids(newroot, &user, &sys).unwrap().ids(),
            (1001, 0),
            "a numeric USER uid:gid must seed at the requested gid, not gid == uid"
        );
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert!(passwd.contains("1001:x:1001:0:"));
        assert!(sys.calls().iter().any(|c| matches!(
            c,
            Call::Lchown {
                uid: 1001,
                gid: 0,
                ..
            }
        )));
    }

    #[test]
    fn workload_ids_resolve_a_named_group_against_etc_group_overriding_the_users_primary() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             app:x:1000:1000:app:/home/app:/bin/sh\n",
        );
        std::fs::write(
            dir.path().join("etc/group"),
            "root:x:0:\nstaff:x:50:\napp:x:1000:\n",
        )
        .unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "app".into(),
            uid: None,
            group: Some("staff".into()),
        };
        assert_eq!(
            resolve_workload_ids(newroot, &user, &sys).unwrap().ids(),
            (1000, 50),
            "an explicitly named group must win over the user's passwd primary gid"
        );
        assert!(
            !sys.calls().iter().any(|c| matches!(c, Call::Lchown { .. })),
            "a passwd-defined user must not be re-seeded just to apply an explicit group"
        );
    }

    #[test]
    fn workload_ids_reject_a_named_group_absent_from_etc_group() {
        let dir = newroot_with_passwd("app:x:1000:1000:app:/home/app:/bin/sh\n");
        std::fs::write(dir.path().join("etc/group"), "root:x:0:\n").unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "app".into(),
            uid: None,
            group: Some("nope".into()),
        };
        assert!(matches!(
            resolve_workload_ids(newroot, &user, &sys),
            Err(MountError::UnresolvableGroup(group)) if group == "nope"
        ));
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
        ReadDir {
            path: String,
        },
        ReadToString {
            path: String,
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
        UnlinkIfSymlink {
            path: String,
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
            uid: u32,
            gid: u32,
        },
        VerifyDigest {
            device: String,
            expected: String,
        },
        ExportEnv {
            key: String,
            value: String,
        },
    }

    type FailWhen = Box<dyn Fn(&Call) -> Option<std::io::ErrorKind>>;

    struct FakeSyscalls {
        calls: RefCell<Vec<Call>>,
        ptmx_exists: bool,
        dir_paths: Vec<String>,
        dir_entries: BTreeMap<String, Vec<ShareEntry>>,
        files: BTreeMap<String, String>,
        /// Symlinks standing in the rootfs before this boot, by the path they stand at and what they point to.
        symlinks: RefCell<BTreeMap<String, String>>,
        fail_when: FailWhen,
        digest_result: RefCell<Option<Result<(), MountError>>>,
    }

    impl FakeSyscalls {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                ptmx_exists: true,
                dir_paths: Vec::new(),
                dir_entries: BTreeMap::new(),
                files: BTreeMap::new(),
                symlinks: RefCell::new(BTreeMap::new()),
                fail_when: Box::new(|_| None),
                digest_result: RefCell::new(None),
            }
        }

        fn with_dir(mut self, path: &str) -> Self {
            self.dir_paths.push(path.to_string());
            self
        }

        fn with_file(mut self, path: &str, body: &str) -> Self {
            self.files.insert(path.to_string(), body.to_string());
            self
        }

        fn with_entries(mut self, path: &str, entries: &[ShareEntry]) -> Self {
            self.dir_entries.insert(path.to_string(), entries.to_vec());
            self
        }

        /// A symlink a previous boot's workload left in the preserved writable layer: `mkdir` and `symlink` fail on it as they do on any existing path, and a write through it lands on what it points to.
        fn with_symlink_left_behind(self, path: &str, points_to: &str) -> Self {
            self.symlinks
                .borrow_mut()
                .insert(path.to_string(), points_to.to_string());
            self
        }

        fn stands_at(&self, path: &str) -> bool {
            self.symlinks.borrow().contains_key(path)
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
            let path = path.to_str().unwrap().to_string();
            let occupied = self.stands_at(&path);
            self.record(Call::Mkdir { path, mode })?;
            match occupied {
                true => Err(std::io::Error::from(ErrorKind::AlreadyExists)),
                false => Ok(()),
            }
        }
        fn symlink(&self, target: &CStr, linkpath: &CStr) -> std::io::Result<()> {
            let linkpath = linkpath.to_str().unwrap().to_string();
            let occupied = self.stands_at(&linkpath);
            self.record(Call::Symlink {
                target: target.to_str().unwrap().to_string(),
                linkpath,
            })?;
            match occupied {
                true => Err(std::io::Error::from(ErrorKind::AlreadyExists)),
                false => Ok(()),
            }
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
        fn is_dir(&self, path: &CStr) -> bool {
            self.dir_paths.iter().any(|p| p == path.to_str().unwrap())
        }

        fn read_to_string(&self, path: &CStr) -> std::io::Result<Option<String>> {
            let path = path.to_string_lossy().into_owned();
            self.record(Call::ReadToString { path: path.clone() })?;
            Ok(self.files.get(&path).cloned())
        }

        fn read_dir(&self, path: &CStr) -> std::io::Result<Vec<ShareEntry>> {
            let path = path.to_string_lossy().into_owned();
            self.record(Call::ReadDir { path: path.clone() })?;
            Ok(self.dir_entries.get(&path).cloned().unwrap_or_default())
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
        fn unlink_if_symlink(&self, path: &CStr) -> std::io::Result<()> {
            let path = path.to_str().unwrap().to_string();
            self.record(Call::UnlinkIfSymlink { path: path.clone() })?;
            self.symlinks.borrow_mut().remove(&path);
            Ok(())
        }
        fn umount(&self, target: &CStr) -> std::io::Result<()> {
            self.record(Call::Umount(target.to_str().unwrap().to_string()))
        }
        fn write_root_file(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()> {
            // O_CREAT|O_TRUNC follows a symlink, so the write lands on what the link points to.
            let landed = self
                .symlinks
                .borrow()
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.to_string());
            self.record(Call::WriteRootFile {
                path: landed,
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
        fn export_env(&self, key: &str, value: &str) {
            self.calls.borrow_mut().push(Call::ExportEnv {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
        fn seed_pristine_volume(
            &self,
            seed_mount: &str,
            image_target: &str,
            uid: u32,
            gid: u32,
        ) -> Result<(), MountError> {
            self.calls.borrow_mut().push(Call::SeedVolume {
                seed_mount: seed_mount.to_string(),
                image_target: image_target.to_string(),
                uid,
                gid,
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
            binds: vec![],
            incomplete_binds: vec![],
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
                flags: MountFlags::none().nosuid().nodev().noexec(),
                data: None,
            })
        );
        // erofs descriptor is mounted read-only and fully hardened (metadata
        // only — never executed, no suid, no dev nodes).
        assert!(calls.contains(&Call::Mount {
            source: "/dev/vdb".into(),
            target: COMPOSEFS_META.into(),
            fstype: "erofs".into(),
            flags: MountFlags::read_only().nosuid().nodev().noexec(),
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
    fn boot_applies_per_mount_hardening_flags() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        let flags_for = |target: &str| -> MountFlags {
            calls
                .iter()
                .find_map(|c| match c {
                    Call::Mount {
                        target: t, flags, ..
                    } if t == target => Some(*flags),
                    _ => None,
                })
                .expect("a mount was recorded for the target")
        };
        // Pseudo filesystems never hold suid bits, device nodes, or executables.
        assert_eq!(flags_for(SYS), MountFlags::none().nosuid().nodev().noexec());
        // /dev and /dev/pts keep device nodes (no nodev) but forbid suid + exec.
        assert_eq!(flags_for(DEV), MountFlags::none().nosuid().noexec());
        assert_eq!(flags_for(DEV_PTS), MountFlags::none().nosuid().noexec());
        // Content store and overlay upper serve the workload's executables, so exec stays on.
        assert_eq!(flags_for(CONTENT), MountFlags::read_only().nosuid().nodev());
        assert_eq!(
            flags_for(UPPER_MOUNTPOINT),
            MountFlags::none().nosuid().nodev()
        );
        // composefs metadata is read-only and fully hardened (never executed).
        assert_eq!(
            flags_for(COMPOSEFS_META),
            MountFlags::read_only().nosuid().nodev().noexec()
        );
        // Pin the permissive rootfs: a regression that hardens it would silently break suid-shipping images.
        assert_eq!(flags_for("/newroot"), MountFlags::none());
    }

    #[test]
    #[should_panic(expected = "bind mount ignores")]
    fn do_mount_rejects_security_flags_on_a_bind_mount() {
        let sys = FakeSyscalls::new();
        let _ = do_mount(
            &sys,
            "/src",
            "/dst",
            "none",
            MountFlags::bind().nosuid(),
            None,
        );
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
            uid: Some(4242),
            group: None,
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
    fn boot_leaves_a_hosts_file_in_the_rootfs_every_session_shares() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        assert_eq!(
            std::fs::read_to_string(format!("{newroot}/etc/hosts")).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n"
        );
    }

    #[test]
    fn a_rootfs_that_refuses_the_hosts_file_still_reaches_the_broker() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("etc"), b"not a directory").unwrap();
        let sys = FakeSyscalls::new();
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            newroot,
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::Syscall { op, .. } if op.contains("fexecve")),
            "a name the guest can still reach by address is not worth refusing the boot: {err:?}"
        );
    }

    #[test]
    fn refuses_to_boot_when_a_named_image_user_resolves_to_no_uid() {
        let dir = newroot_with_passwd("root:x:0:0:root:/root:/bin/sh\n");
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "www-data".into(),
            uid: None,
            group: None,
        };
        let err = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        )
        .unwrap_err();
        assert!(
            matches!(&err, MountError::UnresolvableUser(name) if name == "www-data"),
            "got {err:?}"
        );
        assert!(
            !sys.calls()
                .iter()
                .any(|c| matches!(c, Call::SeedVolume { .. })),
            "volumes must not be seeded as root once the run-as user is unresolvable"
        );
        assert!(
            !sys.calls().iter().any(|c| matches!(c, Call::Chroot(_))),
            "the workload must never be pivoted into when its user can't be resolved"
        );
    }

    fn newroot_with_manifest(dir: &tempfile::TempDir, listing: &str) -> String {
        let newroot = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(format!("{newroot}/.lens")).unwrap();
        std::fs::write(format!("{newroot}{FILESET_OWNED_MANIFEST}"), listing).unwrap();
        newroot
    }

    #[test]
    fn the_workload_owns_the_directories_of_its_tool_trees_but_not_their_files() {
        // A tool writes into its own prefix — cargo's package-cache lock and registry live under a CARGO_HOME the engine put inside the tree — and every injected entry lands uid 0, so a non-root workload could not write there. Directories only: the binaries stay owned by root.
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        for sub in ["rust/1.95.0/home/.cargo", "rust/1.95.0/data/installs"] {
            std::fs::create_dir_all(format!("{newroot}{TOOLS_ROOT}/{sub}")).unwrap();
        }
        std::fs::write(
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0/home/.cargo/env"),
            "x",
        )
        .unwrap();
        let sys = FakeSyscalls::new();
        chown_tool_dirs(&sys, &newroot, Some((65534, 65534)));

        let owned = sys.calls.borrow().clone();
        for dir in [
            format!("{newroot}{TOOLS_ROOT}"),
            format!("{newroot}{TOOLS_ROOT}/rust"),
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0"),
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0/home"),
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0/home/.cargo"),
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0/data"),
            format!("{newroot}{TOOLS_ROOT}/rust/1.95.0/data/installs"),
        ] {
            assert!(
                owned.contains(&handed_over(&dir)),
                "{dir} was not handed to the workload"
            );
        }
        assert!(
            !owned.iter().any(
                |call| matches!(call, Call::Lchown { path, .. } if path.ends_with("/home/.cargo/env"))
            ),
            "a tool's files stay root-owned: {owned:?}"
        );
    }

    /// What the walk is expected to have done to one directory: hand it to the resolved workload ids.
    fn handed_over(path: &str) -> Call {
        Call::Lchown {
            path: path.to_string(),
            uid: 65534,
            gid: 65534,
        }
    }

    #[test]
    fn an_unreadable_tool_directory_costs_its_subtree_and_not_the_rest() {
        // One tool tree the walk cannot descend must not leave every other tool's dirs root-owned.
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        let closed = format!("{newroot}{TOOLS_ROOT}/closed/1.0.0");
        std::fs::create_dir_all(format!("{closed}/inner")).unwrap();
        std::fs::create_dir_all(format!("{newroot}{TOOLS_ROOT}/open/1.0.0")).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();

        let sys = FakeSyscalls::new();
        chown_tool_dirs(&sys, &newroot, Some((65534, 65534)));
        let owned = sys.calls.borrow().clone();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(owned.contains(&handed_over(&format!("{newroot}{TOOLS_ROOT}/open/1.0.0"))));
        assert!(
            owned.contains(&handed_over(&closed)),
            "the dir itself is still handed over"
        );
        assert!(
            !owned.contains(&handed_over(&format!("{closed}/inner"))),
            "its unreadable subtree is skipped: {owned:?}"
        );
    }

    #[test]
    fn a_failed_chown_does_not_abandon_the_remaining_tool_dirs() {
        // The tool trees come from one cache; a single entry the guest cannot take must not silently stop the walk.
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        for tool in ["jq/1.8.2", "node/22.23.2"] {
            std::fs::create_dir_all(format!("{newroot}{TOOLS_ROOT}/{tool}")).unwrap();
        }
        let sys = FakeSyscalls::new().fail_when(|call| {
            matches!(call, Call::Lchown { .. }).then_some(std::io::ErrorKind::PermissionDenied)
        });
        chown_tool_dirs(&sys, &newroot, Some((65534, 65534)));
        assert_eq!(
            sys.calls.borrow().len(),
            5,
            "every dir is attempted: the root, both tools, and both versions"
        );
    }

    #[test]
    fn a_run_with_no_tool_trees_or_no_resolved_user_chowns_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        let sys = FakeSyscalls::new();
        chown_tool_dirs(&sys, &newroot, Some((65534, 65534)));
        assert!(
            sys.calls.borrow().is_empty(),
            "no tool trees, nothing to do"
        );

        std::fs::create_dir_all(format!("{newroot}{TOOLS_ROOT}/jq/1.8.2")).unwrap();
        chown_tool_dirs(&sys, &newroot, None);
        assert!(
            sys.calls.borrow().is_empty(),
            "a root workload already writes its own trees"
        );
    }

    #[test]
    fn a_write_that_lands_after_the_mounts_still_transfers_to_the_workload_user() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(&dir, "/home/node/tool.md\n");
        let staged = format!("{newroot}{}/home/node", crate::staged::STAGED_ROOT);
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(format!("{staged}/tool.md"), b"read me").unwrap();
        let sys = FakeSyscalls::new();

        land_writes_the_mounts_would_have_hidden(&sys, &newroot, &[], &[], Some((1000, 1000)))
            .unwrap();

        assert_eq!(
            std::fs::read(format!("{newroot}/home/node/tool.md")).unwrap(),
            b"read me"
        );
        let landed = Call::Lchown {
            path: format!("{newroot}/home/node/tool.md"),
            uid: 1000,
            gid: 1000,
        };
        assert!(
            sys.calls().contains(&landed),
            "the first chown pass ran before the mount, when this path did not exist yet"
        );
    }

    /// The mask a split bind leaves is guest-local and the writable layer survives a restart, so the boot has to name that bind to the staged landing exactly as it names a volume.
    #[test]
    fn a_staged_write_into_a_split_binds_mask_lands_there_and_not_through_the_last_runs_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        let stands_in_for_the_host = format!("{newroot}/work");
        std::fs::create_dir_all(&stands_in_for_the_host).unwrap();
        let mask = format!("{newroot}/root/.agent");
        std::fs::create_dir_all(&mask).unwrap();
        std::os::unix::fs::symlink(&stands_in_for_the_host, format!("{mask}/session")).unwrap();
        let staged = format!(
            "{newroot}{}/root/.agent/session",
            crate::staged::STAGED_ROOT
        );
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(format!("{staged}/state.json"), b"from-the-document").unwrap();

        let params = CmdlineParams {
            binds: vec![bind_param_seeding(
                "lns-bind-0",
                "/root/.agent",
                &[],
                &["session"],
            )],
            ..full_params()
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &params,
            None,
            &newroot,
            FULL_CMDLINE,
            &FakeSyscalls::new(),
        );

        assert!(
            !std::path::Path::new(&format!("{stands_in_for_the_host}/state.json")).exists(),
            "the link's target stands in for a bound host share, so following it writes the file to the host"
        );
        assert_eq!(
            std::fs::read(format!("{mask}/session/state.json")).unwrap(),
            b"from-the-document",
            "the write belongs in the mask the document named"
        );
    }

    #[test]
    fn a_staged_write_that_cannot_land_refuses_the_boot() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(&dir, "");
        let staged = format!("{newroot}{}/home/node", crate::staged::STAGED_ROOT);
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(format!("{staged}/tool.md"), b"x").unwrap();
        std::fs::create_dir_all(format!("{newroot}/home")).unwrap();
        std::fs::write(format!("{newroot}/home/node"), b"in the way").unwrap();

        let err = land_writes_the_mounts_would_have_hidden(
            &FakeSyscalls::new(),
            &newroot,
            &[],
            &[],
            None,
        )
        .unwrap_err();

        let refusal = format!("{err}");
        assert!(
            refusal.contains("fileset-deferred"),
            "a workload that would start without its file is a boot failure, not a warning"
        );
        assert!(
            refusal.contains("/home/node/tool.md"),
            "a bare errno leaves the operator with no path to look at"
        );
    }

    #[test]
    fn chown_fileset_owned_transfers_each_listed_path_to_the_run_as_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(
            &dir,
            "/home/sandbox\n/home/sandbox/.claude\n/home/sandbox/.claude.json\n",
        );
        let sys = FakeSyscalls::new();
        chown_fileset_owned(&sys, &newroot, Some((65534, 65534)));
        let expected: Vec<Call> = [
            format!("{newroot}/home/sandbox"),
            format!("{newroot}/home/sandbox/.claude"),
            format!("{newroot}/home/sandbox/.claude.json"),
        ]
        .into_iter()
        .map(|path| Call::Lchown {
            path,
            uid: 65534,
            gid: 65534,
        })
        .collect();
        assert_eq!(sys.calls(), expected);
    }

    #[test]
    fn chown_fileset_owned_is_a_noop_without_a_manifest_or_without_run_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap().to_string();
        let sys = FakeSyscalls::new();
        chown_fileset_owned(&sys, &newroot, Some((65534, 65534)));
        // A root workload already owns root-owned files, so a manifest without run ids is also a no-op.
        let with_manifest = newroot_with_manifest(&dir, "/opt/seed/file\n");
        chown_fileset_owned(&sys, &with_manifest, None);
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Lchown { .. })));
    }

    #[test]
    fn chown_fileset_owned_skips_blank_relative_and_traversing_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(&dir, "\nrelative/path\n/ok/../escape\n/kept\n");
        let sys = FakeSyscalls::new();
        chown_fileset_owned(&sys, &newroot, Some((1000, 1000)));
        assert_eq!(
            sys.calls(),
            [Call::Lchown {
                path: format!("{newroot}/kept"),
                uid: 1000,
                gid: 1000,
            }]
        );
    }

    #[test]
    fn chown_fileset_owned_reports_a_failed_lchown_and_keeps_going() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(&dir, "/first\n/second\n");
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Lchown { .. }).then_some(ErrorKind::PermissionDenied));
        chown_fileset_owned(&sys, &newroot, Some((1000, 1000)));
        let attempts = sys
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Lchown { .. }))
            .count();
        assert_eq!(attempts, 2, "one denied path must not abort the rest");
    }

    #[test]
    fn chown_fileset_owned_skips_an_entry_with_an_interior_nul() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = newroot_with_manifest(&dir, "/has\0nul\n/clean\n");
        let sys = FakeSyscalls::new();
        chown_fileset_owned(&sys, &newroot, Some((1000, 1000)));
        assert_eq!(
            sys.calls(),
            [Call::Lchown {
                path: format!("{newroot}/clean"),
                uid: 1000,
                gid: 1000,
            }]
        );
    }

    #[test]
    fn write_sandbox_user_is_idempotent_and_keeps_the_image_entries_intact() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        std::fs::create_dir_all(format!("{newroot}/etc")).unwrap();
        // Existing files lack a trailing newline → the seeded line must not fuse with the image's first entry.
        std::fs::write(
            format!("{newroot}/etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh",
        )
        .unwrap();
        std::fs::write(format!("{newroot}/etc/group"), "root:x:0:").unwrap();
        let sys = FakeSyscalls::new();
        seed_sandbox_user(newroot, "agent", 1000, 1000, &sys);
        seed_sandbox_user(newroot, "agent", 1000, 1000, &sys);
        let passwd = std::fs::read_to_string(format!("{newroot}/etc/passwd")).unwrap();
        assert_eq!(
            passwd.matches("agent:x:1000").count(),
            1,
            "no duplicate user line"
        );
        assert!(
            passwd.ends_with("\nroot:x:0:0:root:/root:/bin/sh"),
            "the image's passwd entries must survive the seeded prepend verbatim: {passwd:?}"
        );
        let group = std::fs::read_to_string(format!("{newroot}/etc/group")).unwrap();
        assert!(
            group.starts_with("agent:x:1000:\n") && group.ends_with("\nroot:x:0:"),
            "the seeded group line must lead and the image's entries must survive: {group:?}"
        );
    }

    #[test]
    fn write_sandbox_user_reports_failed_lchown_without_panicking() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Lchown { .. }).then_some(ErrorKind::PermissionDenied));
        seed_sandbox_user(newroot, "agent", 1000, 1000, &sys);
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
        seed_sandbox_user(newroot, "ag\0ent", 1000, 1000, &sys);
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

    /// A real directory to mount a volume over, plus the ids that own it, so a test can ask for a run-as identity that differs from the first attacher's.
    fn volume_target(newroot: &tempfile::TempDir) -> (String, u32, u32) {
        use std::os::unix::fs::MetadataExt;
        let target = newroot.path().join("data");
        std::fs::create_dir_all(&target).unwrap();
        let md = std::fs::metadata(&target).unwrap();
        (
            newroot.path().to_string_lossy().into_owned(),
            md.uid(),
            md.gid(),
        )
    }

    fn volume_chowns(calls: &[Call]) -> Vec<&Call> {
        calls
            .iter()
            .filter(|c| matches!(c, Call::Lchown { .. }))
            .collect()
    }

    #[test]
    fn a_volume_root_owned_by_another_uid_is_transferred_to_the_run_as_user() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, uid, gid) = volume_target(&dir);
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/data", false)],
            &newroot,
            Some((uid + 1, gid)),
        )
        .unwrap();

        let calls = sys.calls();
        let target = format!("{newroot}/data");
        let chown = calls.iter().position(|c| {
            matches!(c, Call::Lchown { path, uid: u, gid: g } if path == &target && *u == uid + 1 && *g == gid)
        });
        let mount = calls.iter().position(
            |c| matches!(c, Call::Mount { target: t, fstype, .. } if t == &target && fstype == "ext4"),
        );
        assert!(
            chown.is_some(),
            "a volume attached under a different run-as identity leaves every file owned by the first attacher's uid: {calls:?}"
        );
        assert!(
            chown > mount,
            "the chown must land on the volume's own root inode, so it follows the mount: {calls:?}"
        );
    }

    #[test]
    fn a_volume_root_already_owned_by_the_run_as_user_is_not_rewritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, uid, gid) = volume_target(&dir);
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/data", false)],
            &newroot,
            Some((uid, gid)),
        )
        .unwrap();

        assert!(
            volume_chowns(&sys.calls()).is_empty(),
            "the common path costs one stat and no write"
        );
    }

    #[test]
    fn a_root_run_as_leaves_an_image_seeded_volumes_ownership_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, uid, gid) = volume_target(&dir);
        assert_ne!(uid, 0, "the fixture must not already be root-owned");
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/data", false)],
            &newroot,
            Some((0, gid)),
        )
        .unwrap();

        assert!(
            volume_chowns(&sys.calls()).is_empty(),
            "root can write any path, and rewriting here would clobber an image's own ownership"
        );
    }

    #[test]
    fn a_read_only_volume_attach_never_writes_to_the_volume() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, uid, gid) = volume_target(&dir);
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/data", true)],
            &newroot,
            Some((uid + 1, gid)),
        )
        .unwrap();

        assert!(
            volume_chowns(&sys.calls()).is_empty(),
            "a chown on a read-only mount is EROFS, so every :ro attach would log a failure"
        );
    }

    #[test]
    fn a_volume_attach_without_a_resolved_run_as_user_chowns_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, _uid, _gid) = volume_target(&dir);
        let sys = FakeSyscalls::new();

        mount_volumes(&sys, &[volume("/dev/vdc", "/data", false)], &newroot, None).unwrap();

        assert!(volume_chowns(&sys.calls()).is_empty());
    }

    #[test]
    fn a_volume_target_that_cannot_be_stat_ed_is_skipped_without_failing_the_boot() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_string_lossy().into_owned();
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/absent", false)],
            &newroot,
            Some((1000, 1000)),
        )
        .unwrap();

        assert!(volume_chowns(&sys.calls()).is_empty());
    }

    #[test]
    fn a_failed_volume_chown_does_not_abort_the_boot() {
        let dir = tempfile::TempDir::new().unwrap();
        let (newroot, uid, gid) = volume_target(&dir);
        let sys = FakeSyscalls::new().fail_when(|c| {
            matches!(c, Call::Lchown { .. }).then_some(std::io::ErrorKind::PermissionDenied)
        });

        assert!(
            mount_volumes(
                &sys,
                &[volume("/dev/vdc", "/data", false)],
                &newroot,
                Some((uid + 1, gid)),
            )
            .is_ok(),
            "one ownership failure must warn, not strand the run"
        );
    }

    #[test]
    fn every_mounted_volume_target_is_exported_for_the_broker_to_release() {
        let sys = FakeSyscalls::new();

        mount_volumes(
            &sys,
            &[
                volume("/dev/vdc", "/data", false),
                volume("/dev/vdd", "/cache", true),
            ],
            "/newroot",
            None,
        )
        .unwrap();

        let exported = exported_env(&sys.calls());
        assert!(
            exported.contains(&(VOLUME_TARGETS_ENV.to_string(), "/data:/cache".to_string())),
            "the broker cannot release what it was never told about, and a volume left mounted stays marked unchecked: {exported:?}"
        );
    }

    fn exported_env(calls: &[Call]) -> Vec<(String, String)> {
        calls
            .iter()
            .filter_map(|c| match c {
                Call::ExportEnv { key, value } => Some((key.clone(), value.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_run_with_no_volumes_exports_no_target_list() {
        let sys = FakeSyscalls::new();

        mount_volumes(&sys, &[], "/newroot", None).unwrap();

        let exported = exported_env(&sys.calls());
        assert!(
            !exported.iter().any(|(key, _)| key == VOLUME_TARGETS_ENV),
            "a run with no volumes has nothing to release: {exported:?}"
        );
    }

    #[test]
    fn mount_volumes_seeds_each_device_once_then_mounts_every_target_under_newroot() {
        let sys = FakeSyscalls::new();
        let volumes = vec![
            volume("/dev/vdc", "/data", false),
            volume("/dev/vdc", "/cache", true),
        ];
        mount_volumes(&sys, &volumes, "/newroot", Some((1000, 1000))).unwrap();
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
            Call::SeedVolume { image_target, uid: 1000, gid: 1000, .. } if image_target == "/newroot/data"
        ));
        // Seeding mounts the device at the staging path and unmounts it afterward.
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Umount(t) if t == VOLUME_SEED_MOUNT))
        );
        // Both targets are mounted ext4 under newroot, nosuid+nodev; the
        // second is also read-only.
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { target, fstype, flags, .. }
            if target == "/newroot/data" && fstype == "ext4" && *flags == MountFlags::none().nosuid().nodev()))
        );
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { target, flags, .. }
            if target == "/newroot/cache" && *flags == MountFlags::read_only().nosuid().nodev()))
        );
    }

    #[test]
    fn the_upper_mount_also_remounts_read_only_on_error() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );

        let upper = sys
            .calls()
            .into_iter()
            .find(
                |c| matches!(c, Call::Mount { target, fstype, .. } if fstype == "ext4" && target == UPPER_MOUNTPOINT),
            )
            .expect("the upper image is mounted as ext4");
        assert!(
            matches!(&upper, Call::Mount { data, .. } if data.as_deref() == Some(VOLUME_MOUNT_OPTS)),
            "the upper carries a journal like every other image we write, so it takes the same errors= treatment: {upper:?}"
        );
    }

    #[test]
    fn volume_mounts_ask_the_kernel_to_remount_read_only_on_error() {
        let sys = FakeSyscalls::new();
        let volumes = vec![
            volume("/dev/vdc", "/data", false),
            volume("/dev/vdd", "/cache", true),
        ];
        mount_volumes(&sys, &volumes, "/newroot", None).unwrap();

        let ext4: Vec<_> = sys
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Mount { target, fstype, .. } if fstype == "ext4" && target.starts_with("/newroot")))
            .collect();
        assert_eq!(ext4.len(), 2, "one mount per volume: {ext4:?}");
        for call in &ext4 {
            assert!(
                matches!(call, Call::Mount { data, .. } if data.as_deref() == Some(VOLUME_MOUNT_OPTS)),
                "a volume left inconsistent by a killed run keeps taking writes without errors=remount-ro: {call:?}"
            );
        }
    }

    #[test]
    fn the_seed_mount_also_remounts_read_only_on_error() {
        let sys = FakeSyscalls::new();
        seed_volume_if_pristine(&sys, "/dev/vdc", "/newroot/data", None).unwrap();

        let seed = sys
            .calls()
            .into_iter()
            .find(|c| matches!(c, Call::Mount { target, .. } if target == VOLUME_SEED_MOUNT))
            .expect("the seed staging mount happens");
        assert!(
            matches!(&seed, Call::Mount { data, .. } if data.as_deref() == Some(VOLUME_MOUNT_OPTS)),
            "seeding writes to the volume too, so it needs the same protection: {seed:?}"
        );
    }

    fn bind_param(
        tag: &str,
        target: &str,
        read_only: bool,
        drops: &[&str],
    ) -> crate::cmdline::BindParam {
        crate::cmdline::BindParam {
            tag: tag.into(),
            target: target.into(),
            read_only,
            dropped_paths: drops.iter().map(|s| s.to_string()).collect(),
            seeded_paths: Vec::new(),
            excluded_paths: Vec::new(),
        }
    }

    fn bind_param_seeding(
        tag: &str,
        target: &str,
        drops: &[&str],
        seeds: &[&str],
    ) -> crate::cmdline::BindParam {
        crate::cmdline::BindParam {
            seeded_paths: seeds.iter().map(|s| s.to_string()).collect(),
            ..bind_param(tag, target, false, drops)
        }
    }

    fn bind_param_excluding(
        tag: &str,
        target: &str,
        excludes: &[&str],
    ) -> crate::cmdline::BindParam {
        crate::cmdline::BindParam {
            excluded_paths: excludes.iter().map(|s| s.to_string()).collect(),
            ..bind_param(tag, target, false, &[])
        }
    }

    fn dir_entry(name: &str) -> ShareEntry {
        ShareEntry {
            name: name.into(),
            kind: EntryKind::Dir,
        }
    }

    fn file_entry(name: &str) -> ShareEntry {
        ShareEntry {
            name: name.into(),
            kind: EntryKind::File,
        }
    }

    fn mounts_of(sys: &FakeSyscalls) -> Vec<(String, String)> {
        sys.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Mount { source, target, .. } => Some((source, target)),
                _ => None,
            })
            .collect()
    }

    /// The mounts the run itself gets, without the read-only view of the lower stack the boot reads a manifest through and releases again.
    fn run_mounts_of(sys: &FakeSyscalls) -> Vec<(String, String)> {
        mounts_of(sys)
            .into_iter()
            .filter(|(_, target)| target != LOWER_VIEW_MOUNT)
            .collect()
    }

    fn exploded_claude_bind(seeds: &[&str]) -> FakeSyscalls {
        let sys = FakeSyscalls::new()
            .with_entries(
                "/newroot/.lens/binds/0",
                &[
                    dir_entry("projects"),
                    file_entry("settings.json"),
                    dir_entry("agent"),
                ],
            )
            .with_entries(
                "/newroot/.lens/binds/0/agent",
                &[dir_entry("cache"), file_entry("token.json")],
            );
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                seeds,
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the bind explodes rather than covering the seeded path");
        sys
    }

    #[test]
    fn a_seeded_bind_mounts_its_share_out_of_the_way_of_the_target() {
        let sys = exploded_claude_bind(&["settings.json"]);
        let mounts = mounts_of(&sys);
        assert!(
            mounts.contains(&("lns-bind-0".into(), "/newroot/.lens/binds/0".into())),
            "the share lands on a private path, not on the target: {mounts:?}"
        );
        assert!(
            !mounts
                .iter()
                .any(|(_, target)| target == "/newroot/root/.claude"),
            "nothing may cover the target, or the fileset write would reach the share: {mounts:?}"
        );
    }

    #[test]
    fn a_seeded_bind_binds_every_entry_beside_the_seed_and_leaves_the_seed_alone() {
        let sys = exploded_claude_bind(&["settings.json"]);
        let mounts = mounts_of(&sys);
        assert!(
            mounts.contains(&(
                "/newroot/.lens/binds/0/projects".into(),
                "/newroot/root/.claude/projects".into()
            )),
            "a sibling of the seed still reaches the workload: {mounts:?}"
        );
        assert!(
            mounts.contains(&(
                "/newroot/.lens/binds/0/agent".into(),
                "/newroot/root/.claude/agent".into()
            )),
            "a sibling directory is bound whole: {mounts:?}"
        );
        assert!(
            !mounts
                .iter()
                .any(|(_, target)| target == "/newroot/root/.claude/settings.json"),
            "the seeded path is left for the fileset to write: {mounts:?}"
        );
    }

    #[test]
    fn a_seed_below_the_top_level_splits_only_its_own_ancestors() {
        let sys = exploded_claude_bind(&["agent/token.json"]);
        let mounts = mounts_of(&sys);
        assert!(
            !mounts
                .iter()
                .any(|(_, target)| target == "/newroot/root/.claude/agent"),
            "the seed's parent stays a guest-local directory: {mounts:?}"
        );
        assert!(
            mounts.contains(&(
                "/newroot/.lens/binds/0/agent/cache".into(),
                "/newroot/root/.claude/agent/cache".into()
            )),
            "the seed's siblings one level down are bound in: {mounts:?}"
        );
        assert!(
            !mounts
                .iter()
                .any(|(_, target)| target == "/newroot/root/.claude/agent/token.json"),
            "the seed itself is left for the fileset: {mounts:?}"
        );
        assert!(
            mounts.contains(&(
                "/newroot/.lens/binds/0/projects".into(),
                "/newroot/root/.claude/projects".into()
            )),
            "a top-level entry off the spine is still bound whole: {mounts:?}"
        );
    }

    #[test]
    fn a_seeded_bind_masks_its_drops_on_the_share_the_entries_come_from() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[dir_entry("projects")])
            .with_dir("/newroot/.lens/binds/0/.env");
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[".env"],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("masks apply to the private share");
        let mounts = mounts_of(&sys);
        assert!(
            mounts.contains(&("tmpfs".into(), "/newroot/.lens/binds/0/.env".into())),
            "a dropped path is masked where the entries are read from: {mounts:?}"
        );
    }

    #[test]
    fn a_bound_entry_carries_the_masks_under_it_because_a_plain_bind_would_leave_them_behind() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[dir_entry("projects")])
            .with_dir("/newroot/.lens/binds/0/projects/.env");
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &["projects/.env"],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the drop is masked on the share and the entry carries the mask over");
        let recursive = sys.calls().into_iter().any(|call| match call {
            Call::Mount { target, flags, .. } => {
                target == "/newroot/root/.claude/projects" && flags.recursive
            }
            _ => false,
        });
        assert!(
            recursive,
            "a non-recursive bind drops the mask and shows the workload the real secret"
        );
    }

    #[test]
    fn a_symlink_in_the_share_is_recreated_rather_than_followed_out_of_the_share() {
        let sys = FakeSyscalls::new().with_entries(
            "/newroot/.lens/binds/0",
            &[ShareEntry {
                name: "link".into(),
                kind: EntryKind::Symlink("/etc".into()),
            }],
        );
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("a symlink entry is recreated");
        assert!(
            sys.calls().into_iter().any(|call| matches!(
                call,
                Call::Symlink { target, linkpath }
                    if target == "/etc" && linkpath == "/newroot/root/.claude/link"
            )),
            "a whole bind shows a symlink as a symlink, and so must the split"
        );
        assert!(
            !mounts_of(&sys)
                .iter()
                .any(|(source, _)| source == "/newroot/.lens/binds/0/link"),
            "binding what the link resolves to would mount something the document never named"
        );
    }

    #[test]
    fn a_symlink_the_guest_cannot_recreate_refuses_the_boot_rather_than_dropping_it() {
        let sys = FakeSyscalls::new()
            .with_entries(
                "/newroot/.lens/binds/0",
                &[ShareEntry {
                    name: "link".into(),
                    kind: EntryKind::Symlink("/etc".into()),
                }],
            )
            .fail_when(|call| match call {
                Call::Symlink { linkpath, .. } if linkpath == "/newroot/root/.claude/link" => {
                    Some(ErrorKind::PermissionDenied)
                }
                _ => None,
            });
        let err = mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect_err("a link the workload would have seen must not silently vanish");
        assert!(
            format!("{err}").contains("symlink(/etc -> /newroot/root/.claude/link)"),
            "the refusal names the link it could not recreate: {err}"
        );
    }

    #[test]
    fn a_seeded_path_reached_through_a_symlink_refuses_the_boot() {
        let sys = FakeSyscalls::new().with_entries(
            "/newroot/.lens/binds/0",
            &[ShareEntry {
                name: "agent".into(),
                kind: EntryKind::Symlink("/elsewhere".into()),
            }],
        );
        let err = mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent/token.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect_err("following it would write where the document did not name");
        assert!(
            format!("{err}").contains("/newroot/.lens/binds/0/agent"),
            "the refusal names the link it will not follow: {err}"
        );
    }

    #[test]
    fn a_share_the_guest_cannot_read_refuses_the_boot_rather_than_binding_nothing() {
        let sys = FakeSyscalls::new().fail_when(|call| match call {
            Call::ReadDir { path } if path == "/newroot/.lens/binds/0" => {
                Some(ErrorKind::PermissionDenied)
            }
            _ => None,
        });
        let err = mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect_err("an unreadable share must not read as an empty one");
        assert!(
            format!("{err}").contains("read_dir(/newroot/.lens/binds/0)"),
            "the refusal names the share it could not read: {err}"
        );
    }

    #[test]
    fn a_mountpoint_the_guest_cannot_create_refuses_the_boot() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[file_entry("settings.json")])
            .fail_when(|call| match call {
                Call::WriteRootFile { path, .. }
                    if path == "/newroot/root/.claude/settings.json" =>
                {
                    Some(ErrorKind::PermissionDenied)
                }
                _ => None,
            });
        let err = mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect_err("a mountpoint that cannot be made means the entry never reaches the workload");
        assert!(
            format!("{err}").contains("create bind mountpoint /newroot/root/.claude/settings.json"),
            "the refusal names the mountpoint: {err}"
        );
    }

    fn cleared(sys: &FakeSyscalls, landing: &str) -> Option<usize> {
        sys.calls()
            .iter()
            .position(|call| matches!(call, Call::UnlinkIfSymlink { path } if path == landing))
    }

    /// The bind target, and every landing under it, is a directory this boot makes and the previous boot's workload could have replaced with a symlink in the preserved writable layer. Following one would let the workload steer where root writes and mounts.
    #[test]
    fn a_symlink_left_at_an_entry_landing_never_takes_the_write_that_makes_the_mountpoint() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[file_entry("settings.json")])
            .with_symlink_left_behind(
                "/newroot/root/.claude/settings.json",
                "/newroot/work/host-file",
            );
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the landing is cleared and the entry still reaches the workload");
        let written: Vec<String> = sys
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::WriteRootFile { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        assert!(
            !written.contains(&"/newroot/work/host-file".to_string()),
            "creating the mountpoint through the link truncates the host file it points at: {written:?}"
        );
        assert!(
            written.contains(&"/newroot/root/.claude/settings.json".to_string()),
            "the mountpoint is still made at the path the document names: {written:?}"
        );
    }

    /// The split leaves its own symlinks behind in the preserved writable layer, and `symlink(2)` fails on an existing path, so the next boot has to clear the leftover to lay the same link down again.
    #[test]
    fn a_link_this_boot_recreates_replaces_the_one_the_last_boot_left_at_that_landing() {
        let sys = FakeSyscalls::new()
            .with_entries(
                "/newroot/.lens/binds/0",
                &[ShareEntry {
                    name: "link".into(),
                    kind: EntryKind::Symlink("/etc".into()),
                }],
            )
            .with_symlink_left_behind("/newroot/root/.claude/link", "/newroot/work");
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("a restart lays the share's own link down again rather than refusing");
        assert!(
            sys.calls().into_iter().any(|call| matches!(
                call,
                Call::Symlink { linkpath, .. } if linkpath == "/newroot/root/.claude/link"
            )),
            "symlink(2) fails on an existing path, so the leftover has to go first"
        );
    }

    #[test]
    fn a_symlink_left_on_the_spine_goes_before_root_binds_the_entries_below_it() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[dir_entry("agent")])
            .with_entries(
                "/newroot/.lens/binds/0/agent",
                &[file_entry("token.json"), dir_entry("cache")],
            )
            .with_symlink_left_behind("/newroot/root/.claude/agent", "/newroot/work");
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent/token.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the spine is cleared and the split goes on");
        let calls = sys.calls();
        let unlinked =
            cleared(&sys, "/newroot/root/.claude/agent").expect("the spine landing is cleared");
        let bound = calls
            .iter()
            .position(|call| {
                matches!(call, Call::Mount { target, .. } if target == "/newroot/root/.claude/agent/cache")
            })
            .expect("the seed's siblings are still bound in");
        assert!(
            unlinked < bound,
            "root would bind the share's entries wherever the workload pointed: {calls:?}"
        );
    }

    #[test]
    fn a_symlink_left_at_the_target_of_a_split_bind_goes_before_anything_lands_in_it() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[file_entry("settings.json")])
            .with_symlink_left_behind("/newroot/root/.claude", "/newroot/work");
        mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the target is cleared and the split goes on");
        let unlinked = cleared(&sys, "/newroot/root/.claude")
            .expect("the split bind's own target is guest-local, so a symlink there is cleared");
        let mkdir = sys
            .calls()
            .iter()
            .position(
                |call| matches!(call, Call::Mkdir { path, .. } if path == "/newroot/root/.claude"),
            )
            .expect("the target is still made");
        assert!(
            unlinked < mkdir,
            "the whole split lands under this path, so the link goes before the directory is made"
        );
        assert!(
            !sys.calls().into_iter().any(|call| matches!(
                call,
                Call::WriteRootFile { path, .. } if path.starts_with("/newroot/work")
            )),
            "every landing under the target would otherwise be a path the workload chose"
        );
    }

    #[test]
    fn a_bind_mounted_whole_leaves_a_symlink_at_its_target_alone_and_owns_no_landing() {
        let sys = FakeSyscalls::new();
        let split = mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/work", false, &[])],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("an ordinary bind is untouched by the split");
        assert!(
            cleared(&sys, "/newroot/work").is_none(),
            "a whole bind covers whatever stands at its target, and an image may ship a symlink there"
        );
        assert!(
            split.is_empty(),
            "no write is staged past a whole bind, so it owns no landing"
        );
    }

    #[test]
    fn a_symlink_the_boot_cannot_clear_refuses_rather_than_writing_through_it() {
        let sys = FakeSyscalls::new()
            .with_entries("/newroot/.lens/binds/0", &[file_entry("settings.json")])
            .with_symlink_left_behind(
                "/newroot/root/.claude/settings.json",
                "/newroot/work/host-file",
            )
            .fail_when(|call| match call {
                Call::UnlinkIfSymlink { path } if path == "/newroot/root/.claude/settings.json" => {
                    Some(ErrorKind::PermissionDenied)
                }
                _ => None,
            });
        let err = mount_binds(
            &sys,
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["agent"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect_err("a landing that may still be a link is one root must not write");
        assert!(
            format!("{err}")
                .contains("unlink the symlink standing at /newroot/root/.claude/settings.json"),
            "the refusal names the landing: {err}"
        );
    }

    #[test]
    fn a_split_bind_is_named_to_the_staged_writes_so_they_land_where_the_document_says() {
        let split = mount_binds(
            &FakeSyscalls::new().with_entries("/newroot/.lens/binds/0", &[dir_entry("projects")]),
            &[bind_param_seeding(
                "lns-bind-0",
                "/root/.claude",
                &[],
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("the bind splits");
        assert_eq!(
            split,
            ["/root/.claude".to_string()],
            "the mask under a split bind is guest-local, so a symlink the last run left there must not redirect a staged write"
        );
    }

    fn claims_file() -> String {
        format!(
            "{LOWER_VIEW_MOUNT}{}",
            lns_placement::CONNECTOR_WRITES_MANIFEST
        )
    }

    fn tampered_claims_file() -> String {
        format!("/newroot{}", lns_placement::CONNECTOR_WRITES_MANIFEST)
    }

    /// The manifest lives in the runtime layer the host built, and the run's writable layer is preserved across a restart — so a workload that deletes or rewrites its copy must not be the one that decides where a granted file lands.
    #[test]
    fn the_claims_manifest_is_read_from_the_lower_stack_and_not_the_workload_s_copy() {
        let sys = FakeSyscalls::new()
            .with_file(&claims_file(), "claude\t~/.claude/.credentials.json\n")
            .with_file(&tampered_claims_file(), "claude\t~/.claude/projects/x\n")
            .with_entries(
                "/newroot/.lens/binds/0",
                &[file_entry(".credentials.json"), dir_entry("projects")],
            );
        mount_binds(
            &sys,
            &[bind_param_excluding(
                "lns-bind-0",
                "/root/.claude",
                &[".credentials.json"],
            )],
            &[],
            "/newroot",
            None,
            Some("/root"),
        )
        .expect("the manifest the host shipped covers its claim, so the boot stands");
    }

    #[test]
    fn a_manifest_a_workload_deleted_from_the_writable_layer_still_guards_the_boot() {
        let sys = FakeSyscalls::new()
            .with_file(&claims_file(), "claude\t~/.claude/settings.json\n")
            .with_entries("/newroot/.lens/binds/0", &[dir_entry("projects")]);
        mount_binds(
            &sys,
            &[bind_param_excluding(
                "lns-bind-0",
                "/root/.claude",
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            Some("/root"),
        )
        .expect("the claim is covered, so the bind splits");
        assert!(
            mounts_of(&sys).contains(&("lns-bind-0".into(), "/newroot/.lens/binds/0".into())),
            "reading only the writable layer would find no claim, mount the bind whole, and let the granted file reach the host"
        );
    }

    #[test]
    fn the_lower_stack_view_carries_no_writable_layer_and_is_released_before_the_boot_goes_on() {
        let sys = FakeSyscalls::new()
            .with_file(&claims_file(), "claude\t~/.claude/settings.json\n")
            .with_entries("/newroot/.lens/binds/0", &[dir_entry("projects")]);
        mount_binds(
            &sys,
            &[bind_param_excluding(
                "lns-bind-0",
                "/root/.claude",
                &["settings.json"],
            )],
            &[],
            "/newroot",
            None,
            Some("/root"),
        )
        .expect("the claim is covered");
        let view = sys
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Mount { target, data, .. } if target == LOWER_VIEW_MOUNT => Some(data),
                _ => None,
            })
            .flatten()
            .expect("the lower stack is mounted on its own to be read");
        assert!(
            !view.contains("upperdir") && view.contains("lowerdir=/composefs-meta::/content"),
            "an upper layer here is the very layer the workload writes: {view}"
        );
        assert!(
            sys.calls()
                .contains(&Call::Umount(LOWER_VIEW_MOUNT.to_string())),
            "the view is the boot's own, and the workload must not inherit it"
        );
    }

    #[test]
    fn a_path_in_the_lower_view_that_cannot_be_a_c_string_refuses_the_boot() {
        let err = read_from_the_lower_stack(&FakeSyscalls::new(), "/.lens/clai\0ms")
            .expect_err("a path with an interior NUL reaches no file");
        assert!(
            matches!(err, MountError::InteriorNul("lower-view-path")),
            "the refusal names the field: {err}"
        );
    }

    fn ran_with_claims(
        bind: crate::cmdline::BindParam,
        claims: &str,
        home: Option<&str>,
    ) -> (FakeSyscalls, Result<Vec<String>, MountError>) {
        let sys = FakeSyscalls::new()
            .with_file(&claims_file(), claims)
            .with_entries(
                "/newroot/.lens/binds/0",
                &[
                    dir_entry("projects"),
                    file_entry(".credentials.json"),
                    file_entry("settings.json"),
                ],
            );
        let out = mount_binds(&sys, &[bind], &[], "/newroot", None, home);
        (sys, out)
    }

    #[test]
    fn a_connector_claim_an_exclude_covers_leaves_that_entry_unmounted_for_the_grant() {
        let (sys, out) = ran_with_claims(
            bind_param_excluding("lns-bind-0", "/root/.claude", &[".credentials.json"]),
            "claude\t~/.claude/.credentials.json\n",
            Some("/root"),
        );
        out.expect("the exclude covers the claim, so the bind splits instead of refusing");
        let mounts = mounts_of(&sys);
        assert!(
            mounts.contains(&("lns-bind-0".into(), "/newroot/.lens/binds/0".into())),
            "the share goes to the private path, so the claim's parent stays guest-local: {mounts:?}"
        );
        assert!(
            !mounts
                .iter()
                .any(|(_, target)| target == "/newroot/root/.claude/.credentials.json"),
            "the claimed path is left for the grant to write: {mounts:?}"
        );
    }

    #[test]
    fn a_connector_claim_no_exclude_covers_refuses_the_boot_rather_than_writing_to_the_host() {
        let (_, out) = ran_with_claims(
            bind_param_excluding("lns-bind-0", "/root/.claude", &["projects"]),
            "claude\t~/.claude/settings.json\n",
            Some("/root"),
        );
        let err = out.expect_err("a granted file must never reach the share");
        let message = format!("{err}");
        assert!(
            message.contains("/root/.claude/settings.json")
                && message.contains("bind mounted at /root/.claude"),
            "the refusal names the claim and the bind: {message}"
        );
        assert!(message.contains("exclude"), "it names the fix: {message}");
        assert!(
            message.contains("claude"),
            "the developer has to know which connector to act on: {message}"
        );
    }

    #[test]
    fn a_connector_claim_outside_every_bind_changes_nothing() {
        let (sys, out) = ran_with_claims(
            bind_param_excluding("lns-bind-0", "/work", &[]),
            "claude\t~/.claude/.credentials.json\n",
            Some("/root"),
        );
        out.expect("a claim beside the bind is no concern of it");
        assert_eq!(
            run_mounts_of(&sys),
            [("lns-bind-0".to_string(), "/newroot/work".to_string())],
            "no claim under the bind means no split"
        );
    }

    #[test]
    fn a_run_whose_user_has_no_home_counts_no_claim_because_it_can_take_no_such_method() {
        let (sys, out) = ran_with_claims(
            bind_param_excluding("lns-bind-0", "/root/.claude", &["projects"]),
            "claude\t~/.claude/settings.json\n",
            None,
        );
        out.expect("with no home there is nothing to resolve a claim against");
        assert_eq!(
            mounts_of(&sys),
            [(
                "lns-bind-0".to_string(),
                "/newroot/root/.claude".to_string()
            )],
            "the bind is mounted whole, as it would be with no connector installed"
        );
    }

    #[test]
    fn a_document_seed_and_a_connector_claim_split_one_bind_together() {
        let sys = FakeSyscalls::new()
            .with_file(&claims_file(), "claude\t~/.claude/.credentials.json\n")
            .with_entries(
                "/newroot/.lens/binds/0",
                &[
                    dir_entry("projects"),
                    dir_entry("agent"),
                    file_entry(".credentials.json"),
                ],
            );
        let bind = crate::cmdline::BindParam {
            seeded_paths: vec!["agent".into()],
            excluded_paths: vec![".credentials.json".into(), "agent".into()],
            ..bind_param("lns-bind-0", "/root/.claude", false, &[])
        };
        mount_binds(&sys, &[bind], &[], "/newroot", None, Some("/root"))
            .expect("both writers get their path left alone");
        let mounts = mounts_of(&sys);
        assert!(
            mounts.contains(&(
                "/newroot/.lens/binds/0/projects".into(),
                "/newroot/root/.claude/projects".into()
            )),
            "the entries beside both seeds are still bound in: {mounts:?}"
        );
        for left in ["agent", ".credentials.json"] {
            assert!(
                !mounts
                    .iter()
                    .any(|(_, target)| target == &format!("/newroot/root/.claude/{left}")),
                "{left} is left for its writer, not bound from the share: {mounts:?}"
            );
        }
    }

    #[test]
    fn a_claims_manifest_the_guest_cannot_read_refuses_the_boot_rather_than_dropping_the_guard() {
        let sys = FakeSyscalls::new().fail_when(|call| {
            matches!(call, Call::ReadToString { path } if path == &claims_file())
                .then_some(ErrorKind::PermissionDenied)
        });
        let err = mount_binds(
            &sys,
            &[bind_param_excluding("lns-bind-0", "/root/.claude", &[])],
            &[],
            "/newroot",
            None,
            Some("/root"),
        )
        .expect_err("an unreadable manifest is not the same as no connectors installed");
        assert!(
            format!("{err}").contains(&claims_file()),
            "the refusal names what it could not read: {err}"
        );
        assert!(
            !mounts_of(&sys)
                .iter()
                .any(|(source, _)| source == "lns-bind-0"),
            "no share of the run's own stands before the guard is known to hold"
        );
    }

    #[test]
    fn a_manifest_line_the_producer_did_not_write_claims_nothing() {
        let sys =
            FakeSyscalls::new().with_file(&claims_file(), "no-tab-here\nclaude\t/absolute\n\n");
        let claims = connector_claims(&sys, Some("/root")).expect("readable");
        assert!(
            claims.is_empty(),
            "re-anchoring a line this side did not recognise would name a path no grant writes"
        );
    }

    #[test]
    fn a_claim_under_a_volume_refuses_the_boot_because_a_volume_takes_no_exclude() {
        let sys =
            FakeSyscalls::new().with_file(&claims_file(), "claude\t~/.claude/settings.json\n");
        let volume = crate::cmdline::VolumeParam {
            dev: "/dev/vdc".into(),
            target: "/root/.claude".into(),
            read_only: true,
        };
        let err = mount_binds(&sys, &[], &[volume], "/newroot", None, Some("/root"))
            .expect_err("a volume leaves no guest-local room for a granted file");
        let message = format!("{err}");
        assert!(
            message.contains("/root/.claude/settings.json") && message.contains("claude"),
            "the refusal names the claim and the connector: {message}"
        );
        assert!(
            message.contains("volume mounted at") && !message.contains("bind"),
            "naming it a bind would send the developer to a field a volume does not have: {message}"
        );
    }

    #[test]
    fn a_bind_with_no_seed_is_mounted_whole_as_one_mount() {
        let sys = FakeSyscalls::new();
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/work", false, &[])],
            &[],
            "/newroot",
            None,
            None,
        )
        .expect("an ordinary bind is untouched by the split");
        assert_eq!(
            mounts_of(&sys),
            [("lns-bind-0".to_string(), "/newroot/work".to_string())],
            "no seed means no private share and no per-entry binds"
        );
    }

    #[test]
    fn mount_binds_hands_the_workload_every_home_directory_it_had_to_create() {
        // A component left root-owned means the workload can traverse it and write nothing beside what the mount put there.
        let chowned = homes_handed_over("/home/agent", "/home/agent/.claude/projects");
        assert_eq!(
            chowned,
            [
                "/newroot/home/agent",
                "/newroot/home/agent/.claude",
                "/newroot/home/agent/.claude/projects",
            ],
            "the home this boot had to make, and everything it made under it"
        );
    }

    #[test]
    fn a_home_component_the_image_shipped_is_not_this_boots_to_give_away() {
        let sys = FakeSyscalls::new().fail_when(|c| match c {
            Call::Mkdir { path, .. } if path == "/newroot/home/agent" => {
                Some(ErrorKind::AlreadyExists)
            }
            _ => None,
        });
        mount_binds(
            &sys,
            &[bind_param(
                "lns-bind-0",
                "/home/agent/.claude/projects",
                false,
                &[],
            )],
            &[],
            "/newroot",
            Some((1000, 1000)),
            Some("/home/agent"),
        )
        .unwrap();

        let calls = sys.calls();
        let chowned: Vec<&str> = calls
            .iter()
            .filter_map(|c| match c {
                Call::Lchown { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !chowned.contains(&"/newroot/home/agent"),
            "a directory the image already had keeps the owner the image gave it: {chowned:?}"
        );
        assert!(
            chowned.contains(&"/newroot/home/agent/.claude"),
            "and what this boot did make is still handed over: {chowned:?}"
        );
    }

    fn homes_handed_over(home: &str, target: &str) -> Vec<String> {
        let sys = FakeSyscalls::new();
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", target, false, &[])],
            &[],
            "/newroot",
            Some((1000, 1000)),
            Some(home),
        )
        .unwrap();
        sys.calls()
            .iter()
            .filter_map(|c| match c {
                Call::Lchown { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_home_of_slash_names_no_home_so_nothing_is_handed_over() {
        // `nobody` resolves home `/` on some images. Every directory a boot creates is under it, and none of them is that user's home.
        assert!(
            homes_handed_over("/", "/srv/app").is_empty(),
            "a boot-created directory anywhere is not a home"
        );
    }

    #[test]
    fn a_home_written_with_a_trailing_slash_is_the_same_home() {
        // The passwd field is the image's to spell, and a spelling that handed the workload nothing would look exactly like the defect this fixes.
        assert!(
            homes_handed_over("/home/agent/", "/home/agent/.claude/projects")
                .contains(&"/newroot/home/agent".to_string())
        );
    }

    #[test]
    fn mount_binds_mounts_each_tag_as_virtiofs_under_newroot() {
        let sys = FakeSyscalls::new();
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/work", false, &[])],
            &[],
            "/newroot",
            None,
            None,
        )
        .unwrap();
        let calls = sys.calls();
        assert!(
            calls.iter().any(
                |c| matches!(c, Call::Mount { source, target, fstype, flags, .. }
                if source == "lns-bind-0" && target == "/newroot/work" && fstype == "virtiofs"
                && *flags == MountFlags::none().nosuid().nodev())
            ),
            "expected a writable virtiofs mount at /newroot/work: {calls:?}"
        );
    }

    #[test]
    fn mount_binds_marks_a_read_only_bind_read_only() {
        let sys = FakeSyscalls::new();
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/cfg", true, &[])],
            &[],
            "/newroot",
            None,
            None,
        )
        .unwrap();
        assert!(
            sys.calls()
                .iter()
                .any(|c| matches!(c, Call::Mount { target, flags, .. }
                if target == "/newroot/cfg" && *flags == MountFlags::read_only().nosuid().nodev()))
        );
    }

    #[test]
    fn mount_binds_masks_a_dropped_file_with_a_dev_null_bind() {
        let sys = FakeSyscalls::new();
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/work", false, &[".env"])],
            &[],
            "/newroot",
            None,
            None,
        )
        .unwrap();
        let calls = sys.calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { source, target, flags, .. }
                if source == DEV_NULL && target == "/newroot/work/.env" && flags.bind)),
            "a dropped file must be masked by a /dev/null bind: {calls:?}"
        );
    }

    #[test]
    fn mount_binds_masks_a_dropped_directory_with_an_empty_tmpfs() {
        let sys = FakeSyscalls::new().with_dir("/newroot/work/.ssh");
        mount_binds(
            &sys,
            &[bind_param("lns-bind-0", "/work", false, &[".ssh"])],
            &[],
            "/newroot",
            None,
            None,
        )
        .unwrap();
        let calls = sys.calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Mount { target, fstype, data, .. }
                if target == "/newroot/work/.ssh" && fstype == "tmpfs"
                && data.as_deref() == Some("mode=0000,size=4k"))),
            "a dropped directory must be masked by an empty tmpfs: {calls:?}"
        );
    }

    #[test]
    fn mount_volumes_surfaces_a_failed_seed_unmount_with_its_op_string() {
        let sys = FakeSyscalls::new()
            .fail_when(|c| matches!(c, Call::Umount(_)).then_some(ErrorKind::PermissionDenied));
        let err = mount_volumes(
            &sys,
            &[volume("/dev/vdc", "/data", false)],
            "/newroot",
            None,
        )
        .unwrap_err();
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
    fn boot_mounts_dev_shm_as_a_world_writable_tmpfs() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        assert!(
            calls.iter().any(
                |c| matches!(c, Call::Mkdir { path, mode } if path == DEV_SHM && *mode == 0o1777)
            ),
            "devtmpfs ships no shm directory, so the mountpoint has to be created 1777"
        );
        let (flags, data) = calls
            .into_iter()
            .find_map(|c| match c {
                Call::Mount {
                    target,
                    fstype,
                    flags,
                    data,
                    ..
                } if target == DEV_SHM && fstype == "tmpfs" => Some((flags, data)),
                _ => None,
            })
            .expect("/dev/shm is a tmpfs so shm_open and sem_open work");
        assert_eq!(flags, MountFlags::none().nosuid().nodev().noexec());
        assert_eq!(
            data.as_deref(),
            Some("mode=1777"),
            "mode 1777 lets any uid create shm files, and no size= means the kernel default \
             (half of guest RAM) instead of Docker's 64M footgun"
        );
    }

    #[test]
    fn boot_mounts_dev_shm_before_moving_dev_into_the_new_root() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        let shm = calls
            .iter()
            .position(|c| matches!(c, Call::Mount { target, .. } if target == DEV_SHM))
            .expect("/dev/shm is mounted");
        let dev_move = calls
            .iter()
            .position(|c| matches!(c, Call::MoveMount { from, .. } if from == DEV))
            .expect("/dev is moved into the new root");
        assert!(
            shm < dev_move,
            "/dev/shm rides into the new root as a child of the moved /dev subtree, like /dev/pts"
        );
    }

    #[test]
    fn boot_survives_a_dev_shm_mount_failure() {
        let sys = FakeSyscalls::new().fail_when(|c| match c {
            Call::Mount { target, .. } if target == DEV_SHM => Some(ErrorKind::InvalidInput),
            _ => None,
        });
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            sys.calls().iter().any(|c| matches!(c, Call::Fexecve(_))),
            "shared memory is a convenience, not a boot requirement: the run still starts"
        );
    }

    #[test]
    fn boot_mounts_tmp_as_a_boot_scoped_tmpfs() {
        let sys = FakeSyscalls::new();
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            None,
            "/newroot",
            FULL_CMDLINE,
            &sys,
        );
        let (flags, data) = sys
            .calls()
            .into_iter()
            .find_map(|c| match c {
                Call::Mount {
                    target,
                    fstype,
                    flags,
                    data,
                    ..
                } if target == "/newroot/tmp" && fstype == "tmpfs" => Some((flags, data)),
                _ => None,
            })
            .expect("/tmp is mounted as tmpfs so a restart finds it empty");
        assert_eq!(flags, MountFlags::none().nosuid().nodev());
        assert!(
            data.expect("/tmp tmpfs carries options")
                .contains("mode=1777"),
            "/tmp must stay world-writable with the sticky bit"
        );
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
        assert_eq!(
            flags,
            MountFlags::none().nosuid().nodev(),
            "/run must be nosuid,nodev"
        );
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
            MountFlags::none().nosuid().nodev().noexec(),
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
            uid: Some(4242),
            group: None,
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
    fn boot_chowns_run_to_a_named_image_users_passwd_resolved_uid() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             www-data:x:33:33:www-data:/var/www:/usr/sbin/nologin\n",
        );
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "www-data".into(),
            uid: None,
            group: None,
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
                Call::Lchown { path, uid: 33, gid: 33 } if path == &run_target
            )),
            "a named image user's /run must be chowned to its passwd-resolved uid, not left root-owned"
        );
    }

    #[test]
    fn boot_exports_the_passwd_resolved_run_ids_for_the_supervisor_to_drop_to() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             www-data:x:33:50:www-data:/var/www:/usr/sbin/nologin\n",
        );
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "www-data".into(),
            uid: None,
            group: None,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_UID" && value == "33"
            )),
            "the supervisor must drop to the uid lns-init resolved in the guest, not re-resolve the user by name"
        );
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_GID" && value == "50"
            )),
            "the gid must come from the image passwd's gid field, not from a like-named group or the uid"
        );
        let run_target = format!("{newroot}/run");
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::Lchown { path, uid: 33, gid: 50 } if path == &run_target
            )),
            "/run must be group-owned by the workload's passwd gid, not by the uid"
        );
    }

    #[test]
    fn boot_exports_the_passwd_resolved_home_for_exec_sessions_to_adopt() {
        let dir = newroot_with_passwd(
            "root:x:0:0:root:/root:/bin/sh\n\
             www-data:x:33:50:www-data:/var/www:/usr/sbin/nologin\n",
        );
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "www-data".into(),
            uid: None,
            group: None,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            sys.calls().iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_HOME" && value == "/var/www"
            )),
            "an exec session adopts the workload user's home, so the broker needs the passwd home lns-init already read"
        );
    }

    #[test]
    fn the_exported_home_comes_from_the_same_passwd_line_as_the_run_ids() {
        let dir = newroot_with_passwd(
            "sandbox:x:notanum:0:decoy:/opt/attacker:/bin/sh\n\
             sandbox:x:1000:1000::/home/sandbox:/bin/sh\n",
        );
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "sandbox".into(),
            uid: None,
            group: None,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_UID" && value == "1000"
            )),
            "the malformed first line must not satisfy the uid lookup"
        );
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_HOME" && value == "/home/sandbox"
            )),
            "the home must come from the line that supplied the run ids, never from a sibling line the id parse rejected"
        );
        assert!(
            !calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_HOME" && value == "/opt/attacker"
            )),
            "a decoy line must not hand the exec a different identity's home"
        );
    }

    #[test]
    fn a_passwd_home_with_control_bytes_is_never_exported() {
        let dir = newroot_with_passwd("sandbox:x:1000:1000::/ho\u{0}me:/bin/sh\n");
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "sandbox".into(),
            uid: None,
            group: None,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, .. } if key == "LENS_RUN_UID"
            )),
            "the run ids still export; only the poisoned home is dropped"
        );
        assert!(
            !calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, .. } if key == "LENS_RUN_HOME"
            )),
            "a NUL in the home field would panic std::env::set_var in PID 1 — the export must be skipped"
        );
    }

    #[test]
    fn boot_exports_the_seeded_users_home_when_the_image_passwd_lacks_the_user() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "1001".into(),
            uid: Some(1001),
            group: None,
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        assert!(
            sys.calls().iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_HOME" && value == "/home/1001"
            )),
            "a seeded user's home is the /home/<name> lns-init creates, and an exec must land there too"
        );
    }

    #[test]
    fn boot_exports_the_explicit_image_group_not_the_seeded_users_uid() {
        let dir = tempfile::TempDir::new().unwrap();
        let newroot = dir.path().to_str().unwrap();
        let sys = FakeSyscalls::new();
        let user = SandboxUser {
            name: "1001".into(),
            uid: Some(1001),
            group: Some("0".into()),
        };
        let _ = mount_composefs_and_exec_broker_inner(
            &full_params(),
            Some(&user),
            newroot,
            FULL_CMDLINE,
            &sys,
        );
        let calls = sys.calls();
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::ExportEnv { key, value } if key == "LENS_RUN_GID" && value == "0"
            )),
            "USER 1001:0 must run with gid 0, not the gid==uid fallback that drops the group"
        );
        let run_target = format!("{newroot}/run");
        assert!(
            calls.iter().any(|c| matches!(
                c,
                Call::Lchown { path, uid: 1001, gid: 0 } if path == &run_target
            )),
            "/run must be group-owned by the explicitly requested gid 0"
        );
    }

    #[test]
    fn boot_exports_no_run_ids_for_a_rootful_image_so_the_supervisor_keeps_root() {
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
                .any(|c| matches!(c, Call::ExportEnv { .. })),
            "with no sandbox user the supervisor must see no run-as ids and keep the workload as root"
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
            uid: Some(4242),
            group: None,
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
                if target == "/newroot/proc/cmdline" && *flags == MountFlags::bind())
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
                if target == "/newroot/proc/cmdline" && *flags == MountFlags::bind())
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
