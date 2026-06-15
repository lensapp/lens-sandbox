use anyhow::{Context, Result};
use clap::FromArgMatches;
use lns_ipc::{PlatformInfo, Uname, shell_basename_from, uname_fields_with};

use crate::command::{RunCtx, RunFuture};
use crate::service::real::RealServiceClient;
use crate::update::UpdateArgs;

pub fn run_command<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = UpdateArgs::from_arg_matches(matches)?;
        run(args).await
    })
}

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
                let cstr = std::ffi::CStr::from_ptr(arr.as_ptr());
                cstr.to_string_lossy().into_owned()
            };
            Some((
                to_string(&buf.sysname),
                to_string(&buf.machine),
                to_string(&buf.release),
            ))
        }
    }
}

pub async fn run(args: UpdateArgs) -> Result<i32> {
    if args.dry_run {
        crate::update_check::real::run_dry_run()?;
        return Ok(0);
    }
    let lns_path = std::env::current_exe().context("resolving current `lns` executable path")?;
    let service = RealServiceClient::new(
        crate::service::socket_path()?,
        crate::service::find_service_binary(),
    );
    super::run_with(
        args,
        super::LNS_VERSION,
        super::DEFAULT_CDN_BASE,
        &detect_platform(),
        &lns_path,
        &service,
    )
    .await
}

fn detect_platform() -> PlatformInfo {
    let (os, arch, kernel_release) =
        uname_fields_with(&RealUname, std::env::consts::OS, std::env::consts::ARCH);
    PlatformInfo {
        os,
        arch,
        kernel_release,
        shell: shell_basename_from(std::env::var_os("SHELL")),
    }
}
