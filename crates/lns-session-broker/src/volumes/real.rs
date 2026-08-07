#![cfg(target_os = "linux")]

use super::{Released, VolumeSyscalls, release_volumes_with, targets_from_env};

#[derive(Default)]
struct RealVolumeSyscalls;

impl VolumeSyscalls for RealVolumeSyscalls {
    fn umount(&self, target: &str) -> std::io::Result<()> {
        let path = std::ffi::CString::new(target)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: `path` is a valid NUL-terminated C string that outlives the call.
        match unsafe { libc::umount(path.as_ptr()) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    fn remount_read_only(&self, target: &str) -> std::io::Result<()> {
        let path = std::ffi::CString::new(target)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: `path` is a valid NUL-terminated C string that outlives the call; a NULL source/fstype/data is what MS_REMOUNT takes.
        match unsafe {
            libc::mount(
                std::ptr::null(),
                path.as_ptr(),
                std::ptr::null(),
                libc::MS_REMOUNT | libc::MS_RDONLY,
                std::ptr::null(),
            )
        } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}

pub fn release_volumes() {
    let targets = targets_from_env(std::env::var(super::TARGETS_ENV).ok());
    for (target, released) in release_volumes_with(&RealVolumeSyscalls, &targets) {
        if let Released::Failed(reason) = released {
            eprintln!("lns-session-broker: {target} not released cleanly: {reason}");
        }
    }
}
