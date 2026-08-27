use lns_ipc::{PlatformInfo, Uname, shell_basename_from, uname_fields_with};

struct RealUname;

impl Uname for RealUname {
    fn uname(&self) -> Option<(String, String, String)> {
        // SAFETY: `libc::uname` writes into a zeroed `utsname` and the returned C strings live as long as the struct on our stack.
        unsafe {
            let mut buf: libc::utsname = std::mem::zeroed();
            if libc::uname(&mut buf) != 0 {
                return None;
            }
            let to_string = |arr: &[libc::c_char]| {
                std::ffi::CStr::from_ptr(arr.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            };
            Some((
                to_string(&buf.sysname),
                to_string(&buf.machine),
                to_string(&buf.release),
            ))
        }
    }
}

pub fn detect() -> PlatformInfo {
    let (os, arch, kernel_release) =
        uname_fields_with(&RealUname, std::env::consts::OS, std::env::consts::ARCH);
    PlatformInfo {
        os,
        arch,
        kernel_release,
        shell: shell_basename_from(std::env::var_os("SHELL")),
    }
}
