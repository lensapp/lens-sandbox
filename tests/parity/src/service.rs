use crate::lns::{Lns, parse_status_pid, pid_alive, wait_until};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STOP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct PrivateService {
    lns: Lns,
    home: PathBuf,
    socket: PathBuf,
    pid: Option<u32>,
    stopped: bool,
}

impl PrivateService {
    pub fn start(
        lns_bin: &Path,
        service_bin: &Path,
        backend_env: &BTreeMap<String, String>,
        root: &Path,
    ) -> Result<Self> {
        let home = root.join("home");
        let socket = root.join("service.sock");
        std::fs::create_dir_all(&home).with_context(|| format!("create {}", home.display()))?;

        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), home.display().to_string());
        env.insert(
            "LNS_HOME".to_string(),
            home.join(".lns").display().to_string(),
        );
        env.insert("LNS_SOCKET_PATH".to_string(), socket.display().to_string());
        env.insert(
            "LNS_SERVICE_BIN".to_string(),
            absolute(service_bin)?.display().to_string(),
        );
        env.insert("LNS_HEADLESS".to_string(), "1".to_string());
        env.extend(backend_env.clone());

        let lns = Lns::new(absolute(lns_bin)?, env);
        let started = lns.run(&["service", "start"])?;
        if !started.ok() {
            bail!(
                "`lns service start` failed on the private service: {}",
                started.combined().trim()
            );
        }

        let mut service = Self {
            lns,
            home,
            socket,
            pid: None,
            stopped: false,
        };
        service.pid = service.read_pid()?;
        if service.pid.is_none() {
            bail!("the private service started but reported no PID");
        }
        Ok(service)
    }

    pub fn lns(&self) -> &Lns {
        &self.lns
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn read_pid(&self) -> Result<Option<u32>> {
        Ok(parse_status_pid(
            &self.lns.run(&["service", "status"])?.stdout,
        ))
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let stopped = self.lns.run(&["service", "stop"])?;
        if !stopped.ok() {
            bail!(
                "`lns service stop` failed on the private service: {}",
                stopped.combined().trim()
            );
        }
        if let Some(pid) = self.pid
            && !wait_until(STOP_TIMEOUT, || !pid_alive(pid))
        {
            bail!("the private service {pid} is still alive after `lns service stop`");
        }
        Ok(())
    }
}

impl Drop for PrivateService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !candidate.exists() {
        bail!("no binary at {}", candidate.display());
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_the_person_named_must_exist_before_a_run_starts() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("lns");
        let err = absolute(&missing).unwrap_err().to_string();
        assert!(err.contains(&missing.display().to_string()), "{err}");

        std::fs::write(&missing, b"#!/bin/sh\n").unwrap();
        assert_eq!(absolute(&missing).unwrap(), missing);
    }

    #[test]
    fn a_relative_binary_is_resolved_against_the_directory_the_harness_runs_in() {
        let err = absolute(Path::new("no-such-binary"))
            .unwrap_err()
            .to_string();
        let here = std::env::current_dir().unwrap();

        assert!(err.contains(&here.display().to_string()), "{err}");
        assert!(err.contains("no-such-binary"), "{err}");
    }

    #[test]
    fn a_service_that_never_started_is_reported_rather_than_used() {
        let dir = tempfile::tempdir().unwrap();
        let fake_lns = dir.path().join("lns");
        std::fs::write(
            &fake_lns,
            "#!/bin/sh\necho 'no service today' >&2\nexit 1\n",
        )
        .unwrap();
        set_executable(&fake_lns);
        let service_bin = dir.path().join("lns-service");
        std::fs::write(&service_bin, b"").unwrap();

        let err = PrivateService::start(&fake_lns, &service_bin, &BTreeMap::new(), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no service today"), "{err}");
    }

    #[test]
    fn a_service_that_reports_no_pid_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fake_lns = dir.path().join("lns");
        std::fs::write(&fake_lns, "#!/bin/sh\necho 'LNS is running.'\n").unwrap();
        set_executable(&fake_lns);
        let service_bin = dir.path().join("lns-service");
        std::fs::write(&service_bin, b"").unwrap();

        let err = PrivateService::start(&fake_lns, &service_bin, &BTreeMap::new(), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no PID"), "{err}");
    }

    #[test]
    fn the_private_service_runs_on_its_own_home_socket_and_backend_environment() {
        let dir = tempfile::tempdir().unwrap();
        let fake_lns = dir.path().join("lns");
        std::fs::write(
            &fake_lns,
            "#!/bin/sh\nif [ \"$2\" = status ]; then echo '  PID: 4242'; fi\n",
        )
        .unwrap();
        set_executable(&fake_lns);
        let service_bin = dir.path().join("lns-service");
        std::fs::write(&service_bin, b"").unwrap();

        let mut service = PrivateService::start(
            &fake_lns,
            &service_bin,
            &BTreeMap::from([("LNS_NETDEV".to_string(), "netstack".to_string())]),
            dir.path(),
        )
        .unwrap();

        assert_eq!(service.pid(), Some(4242));
        assert_eq!(service.home(), dir.path().join("home"));
        assert_eq!(service.socket(), dir.path().join("service.sock"));
        let env = &service.lns().env;
        assert_eq!(env["LNS_HEADLESS"], "1");
        assert_eq!(env["LNS_NETDEV"], "netstack");
        assert_eq!(
            env["LNS_SOCKET_PATH"],
            service.socket().display().to_string()
        );
        assert!(env["LNS_HOME"].ends_with(".lns"), "{}", env["LNS_HOME"]);

        service.pid = None;
        service.stop().unwrap();
    }

    #[test]
    fn a_service_that_refuses_to_stop_is_reported_not_left_behind_silently() {
        let dir = tempfile::tempdir().unwrap();
        let fake_lns = dir.path().join("lns");
        std::fs::write(&fake_lns, "#!/bin/sh\necho 'still here' >&2\nexit 1\n").unwrap();
        set_executable(&fake_lns);

        let mut service = PrivateService {
            lns: Lns::new(fake_lns, BTreeMap::new()),
            home: dir.path().join("home"),
            socket: dir.path().join("service.sock"),
            pid: Some(std::process::id()),
            stopped: false,
        };
        let err = service.stop().unwrap_err().to_string();
        assert!(err.contains("still here"), "{err}");

        assert!(service.stop().is_ok(), "a second stop is a no-op");
    }

    fn set_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
