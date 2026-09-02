use std::ffi::CStr;
use std::io;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::ptr;

use super::{MountError, MountFlags, NEWROOT, PROC, Syscalls};
use crate::cmdline::CmdlineParams;

struct RealSyscalls;

fn rc_to_result(rc: c_int) -> io::Result<()> {
    if rc != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Syscalls for RealSyscalls {
    fn mount(
        &self,
        source: &CStr,
        target: &CStr,
        fstype: &CStr,
        flags: MountFlags,
        data: Option<&CStr>,
    ) -> io::Result<()> {
        let mut f: libc::c_ulong = 0;
        if flags.bind {
            f |= libc::MS_BIND;
        }
        if flags.recursive {
            f |= libc::MS_REC;
        }
        if flags.read_only {
            f |= libc::MS_RDONLY;
        }
        if flags.nosuid {
            f |= libc::MS_NOSUID;
        }
        if flags.nodev {
            f |= libc::MS_NODEV;
        }
        if flags.noexec {
            f |= libc::MS_NOEXEC;
        }
        let data_ptr = data
            .map(|c| c.as_ptr() as *const libc::c_void)
            .unwrap_or(ptr::null());
        // SAFETY: all CStr pointers are valid for the duration of this call.
        let rc = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                f,
                data_ptr,
            )
        };
        rc_to_result(rc)
    }

    fn move_mount(&self, from: &CStr, to: &CStr) -> io::Result<()> {
        // SAFETY: from/to are valid NUL-terminated C strings for this call.
        let rc = unsafe {
            libc::mount(
                from.as_ptr(),
                to.as_ptr(),
                ptr::null(),
                libc::MS_MOVE,
                ptr::null(),
            )
        };
        rc_to_result(rc)
    }

    fn mkdir(&self, path: &CStr, mode: u32) -> io::Result<()> {
        // SAFETY: path is a valid NUL-terminated C string.
        let rc = unsafe { libc::mkdir(path.as_ptr(), mode as libc::mode_t) };
        rc_to_result(rc)
    }

    fn symlink(&self, target: &CStr, linkpath: &CStr) -> io::Result<()> {
        // SAFETY: both are valid NUL-terminated C strings.
        let rc = unsafe { libc::symlink(target.as_ptr(), linkpath.as_ptr()) };
        rc_to_result(rc)
    }

    fn mknod_char(&self, path: &CStr, mode: u32, major: u32, minor: u32) -> io::Result<()> {
        let dev = libc::makedev(major, minor);
        let m = libc::S_IFCHR | mode as libc::mode_t;
        // SAFETY: path is a valid NUL-terminated C string.
        let rc = unsafe { libc::mknod(path.as_ptr(), m, dev) };
        rc_to_result(rc)
    }

    fn chroot(&self, path: &CStr) -> io::Result<()> {
        // SAFETY: path is a valid NUL-terminated C string.
        let rc = unsafe { libc::chroot(path.as_ptr()) };
        rc_to_result(rc)
    }

    fn chdir(&self, path: &CStr) -> io::Result<()> {
        // SAFETY: path is a valid NUL-terminated C string.
        let rc = unsafe { libc::chdir(path.as_ptr()) };
        rc_to_result(rc)
    }

    fn path_exists(&self, path: &CStr) -> bool {
        // SAFETY: all-zero is a valid libc::stat; stat(2) overwrites it and only reads *path.
        unsafe {
            let mut buf = std::mem::zeroed::<libc::stat>();
            libc::stat(path.as_ptr(), &mut buf) == 0
        }
    }

    fn is_dir(&self, path: &CStr) -> bool {
        // SAFETY: all-zero is a valid libc::stat; stat(2) overwrites it and only reads *path.
        unsafe {
            let mut buf = std::mem::zeroed::<libc::stat>();
            libc::stat(path.as_ptr(), &mut buf) == 0
                && (buf.st_mode & libc::S_IFMT) == libc::S_IFDIR
        }
    }

    fn read_to_string(&self, path: &CStr) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(std::path::Path::new(std::ffi::OsStr::from_bytes(
            path.to_bytes(),
        ))) {
            Ok(body) => Ok(Some(body)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn read_dir(&self, path: &CStr) -> std::io::Result<Vec<super::ShareEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(std::path::Path::new(std::ffi::OsStr::from_bytes(
            path.to_bytes(),
        )))? {
            let entry = entry?;
            let kind = match std::fs::symlink_metadata(entry.path())? {
                meta if meta.is_symlink() => super::EntryKind::Symlink(
                    std::fs::read_link(entry.path())?
                        .to_string_lossy()
                        .into_owned(),
                ),
                meta if meta.is_dir() => super::EntryKind::Dir,
                _ => super::EntryKind::File,
            };
            entries.push(super::ShareEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn open_ro(&self, path: &CStr) -> io::Result<RawFd> {
        // SAFETY: path is a valid NUL-terminated C string.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn fexecve(&self, fd: RawFd, argv0: &CStr) -> io::Error {
        // SAFETY: linker-provided `environ` is the process env; argv is NUL-terminated; fd is O_RDONLY.
        unsafe {
            unsafe extern "C" {
                static environ: *const *const c_char;
            }
            let argv: [*const c_char; 2] = [argv0.as_ptr(), ptr::null()];
            libc::fexecve(fd, argv.as_ptr(), environ);
        }
        io::Error::last_os_error()
    }

    fn lchown(&self, path: &CStr, uid: u32, gid: u32) -> io::Result<()> {
        // SAFETY: path is a valid NUL-terminated C string; lchown reads it only.
        let rc = unsafe { libc::lchown(path.as_ptr(), uid, gid) };
        rc_to_result(rc)
    }

    fn umount(&self, target: &CStr) -> io::Result<()> {
        // SAFETY: target is a valid NUL-terminated C string; umount reads it only.
        let rc = unsafe { libc::umount(target.as_ptr()) };
        rc_to_result(rc)
    }

    fn write_root_file(&self, path: &str, contents: &[u8], mode: u32) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.write_all(contents)
    }

    fn write_sysctl(&self, path: &str, value: &str) -> io::Result<()> {
        std::fs::write(path, value)
    }

    fn seed_pristine_volume(
        &self,
        seed_mount: &str,
        image_target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), MountError> {
        crate::seed::prepare_pristine_volume(
            std::path::Path::new(seed_mount),
            std::path::Path::new(image_target),
            uid,
            gid,
        )
        .map(|_| ())
        .map_err(|err| MountError::Syscall {
            op: format!("prepare pristine volume {image_target} -> {seed_mount}"),
            err,
        })
    }

    fn verify_descriptor_digest(
        &self,
        device_path: &str,
        expected_hex: &str,
    ) -> Result<(), MountError> {
        super::verify_descriptor_digest(device_path, expected_hex)
    }

    fn export_env(&self, key: &str, value: &str) {
        // SAFETY: lns-init is single-threaded PID-1 here; nothing else can race the environment before the imminent fexecve into the broker.
        unsafe { std::env::set_var(key, value) };
    }
}

pub fn mount_and_exec() -> Result<std::convert::Infallible, MountError> {
    let sys = RealSyscalls;
    super::do_mount_pseudo(
        &sys,
        PROC,
        "proc",
        super::MountFlags::none().nosuid().nodev().noexec(),
    )?;

    let cmdline = std::fs::read_to_string("/proc/cmdline").map_err(MountError::DescriptorRead)?;
    let params = CmdlineParams::parse(&cmdline);
    super::validate_cmdline(&params)?;

    let user = super::resolve_sandbox_user(|k| std::env::var(k).ok());
    super::mount_composefs_and_exec_broker_inner(&params, user.as_ref(), NEWROOT, &cmdline, &sys)
}
