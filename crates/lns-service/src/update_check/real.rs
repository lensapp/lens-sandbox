use super::UpdateStatus;
use super::traits::{Clock, Fetcher, StateStore};
use crate::http_cap::CappedBuffer;
use crate::shutdown::Shutdown;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use lns_ipc::{Method, PlatformInfo, Uname, shell_basename_from, uname_fields_with};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

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

fn detect_user_agent() -> String {
    let (os, arch, kernel_release) =
        uname_fields_with(&RealUname, std::env::consts::OS, std::env::consts::ARCH);
    let platform = PlatformInfo {
        os,
        arch,
        kernel_release,
        shell: shell_basename_from(std::env::var_os("SHELL")),
    };
    lns_ipc::user_agent(super::LNS_VERSION, &platform, Method::ServiceUpdateCheck)
}

struct RealClock;
impl Clock for RealClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

struct RealFetcher;
impl Fetcher for RealFetcher {
    async fn get_manifest(&self, url: &str, user_agent: &str, install_id: &str) -> Result<Vec<u8>> {
        let resp = reqwest::Client::new()
            .get(url)
            .timeout(FETCH_TIMEOUT)
            .header("user-agent", user_agent)
            .query(&[("install_id", install_id)])
            .send()
            .await
            .with_context(|| format!("checking {url}"))?
            .error_for_status()
            .with_context(|| format!("HTTP error from {url}"))?;
        let mut buf = CappedBuffer::new(resp.content_length(), super::MAX_MANIFEST_BYTES, url)?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.push(chunk.map(|b| b.to_vec()), url)?;
        }
        Ok(buf.into_inner())
    }
}

struct RealStore;
impl RealStore {
    fn root() -> Result<PathBuf> {
        Ok(lns_ipc::data_root()?)
    }
}
impl StateStore for RealStore {
    fn read_install_id(&self) -> Result<Option<String>> {
        let path = Self::root()?.join("install-id");
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let t = s.trim();
                Ok((!t.is_empty()).then(|| t.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
    fn write_install_id(&self, id: &str) -> Result<()> {
        let root = Self::root()?;
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("install-id"), id).context("writing install-id")
    }
    fn write_status(&self, status: &UpdateStatus) -> Result<()> {
        let root = Self::root()?;
        std::fs::create_dir_all(&root)?;
        let json = serde_json::to_string(status).context("serializing update status")?;
        std::fs::write(root.join("update-status.json"), json).context("writing update status")
    }
}

pub async fn run_periodic(shutdown: Arc<Shutdown>) {
    if let Err(e) = super::establish_install_id(|k| std::env::var(k).ok(), &RealStore) {
        crate::log::debug!("establishing install id failed: {e:#}");
    }
    let user_agent = detect_user_agent();
    let first_tick = tokio::time::Instant::now() + super::CHECK_INTERVAL;
    let mut tick = tokio::time::interval_at(first_tick, super::CHECK_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(e) = super::check_once(
                    |k| std::env::var(k).ok(),
                    &RealFetcher,
                    &RealStore,
                    &RealClock,
                    super::CDN_BASE,
                    &user_agent,
                )
                .await
                {
                    crate::log::debug!("update check failed: {e:#}");
                }
            }
            _ = shutdown.wait_async() => break,
        }
    }
}
