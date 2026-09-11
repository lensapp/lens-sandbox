#[cfg(any(target_os = "linux", test))]
mod workload_env;

#[cfg(any(target_os = "linux", test))]
mod terminal;

#[cfg(any(target_os = "linux", test))]
mod pre_start;

#[cfg(any(target_os = "linux", test))]
mod lifecycle;

#[cfg(any(target_os = "linux", test))]
mod reaper;

#[cfg(target_os = "linux")]
mod openshell_guest;

#[cfg(target_os = "linux")]
mod proxy;

#[cfg(any(target_os = "linux", test))]
mod dns_config;

#[cfg(target_os = "linux")]
mod dns_guest;

#[cfg(target_os = "linux")]
mod script_runner;

#[cfg(target_os = "linux")]
mod signals;

#[cfg(target_os = "linux")]
mod isolation;

#[cfg(target_os = "linux")]
mod exec_guest;

#[cfg(target_os = "linux")]
mod workload;

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();
    let named = lifecycle::name(|name| {
        let name = std::ffi::CString::new(name)?;
        // SAFETY: prctl copies the nul-terminated name from the live CString into this process.
        if unsafe { libc::prctl(libc::PR_SET_NAME, name.as_ptr(), 0, 0, 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    });
    let result = if let Err(error) = named {
        Err(error.into())
    } else if std::env::args().nth(1).as_deref() == Some(lns_session::isolation::EXEC_MODE) {
        exec_guest::run().await
    } else {
        openshell_guest::run().await
    };
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            crate::log::error!("OpenShell supervisor refused workload: {error}");
            125
        }
    };
    std::process::exit(code);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("lns-supervisor requires the Linux guest");
    std::process::exit(125);
}

#[cfg(target_os = "linux")]
mod log {
    pub use tracing::error;
}
