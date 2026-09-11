use super::ROOT_CAPABILITIES as CAPABILITIES;

#[cfg(target_os = "linux")]
pub fn apply(gid: u32) -> std::io::Result<()> {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    // SAFETY: these scalar syscalls restrict only the forked child's identity and privileges.
    unsafe {
        if libc::setgroups(0, std::ptr::null()) != 0
            || libc::setgid(gid) != 0
            || libc::setresuid(0, 0, 0) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        for cap in 0..64 {
            if CAPABILITIES & (1u64 << cap) == 0
                && libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) != 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINVAL)
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        let header = Header {
            version: 0x20080522,
            pid: 0,
        };
        let data = [
            Data {
                effective: CAPABILITIES as u32,
                permitted: CAPABILITIES as u32,
                inheritable: 0,
            },
            Data {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        if libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        ) != 0
            || libc::syscall(libc::SYS_capset, &header, data.as_ptr()) != 0
            || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}
