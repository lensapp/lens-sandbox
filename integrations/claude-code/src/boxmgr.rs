use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxState {
    Running,
    Stopped,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSpec {
    pub name: String,
    pub image: String,
    pub mount: String,
    pub workdir: String,
}

pub trait LnsRunner {
    fn inspect(&self, name: &str) -> Result<BoxState, String>;
    fn boot(&self, spec: &BoxSpec) -> Result<(), String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    fn exec(&self, name: &str, argv: &[String]) -> Result<i32, String>;
}

pub fn box_name(runtime_key: &str, cwd: &str) -> String {
    format!("cc-{runtime_key}-{}", short_hash(cwd))
}

pub fn box_spec(runtime_key: &str, image: &str, cwd: &str) -> BoxSpec {
    BoxSpec {
        name: box_name(runtime_key, cwd),
        image: image.to_string(),
        mount: format!("{cwd}:{cwd}"),
        workdir: cwd.to_string(),
    }
}

pub fn ensure_box<R: LnsRunner>(runner: &R, spec: &BoxSpec) -> Result<(), String> {
    match runner.inspect(&spec.name)? {
        BoxState::Running => Ok(()),
        BoxState::Stopped => {
            runner.remove(&spec.name)?;
            runner.boot(spec)
        }
        BoxState::NotFound => runner.boot(spec),
    }
}

pub fn run_in_box<R: LnsRunner>(
    runner: &R,
    runtime_key: &str,
    image: &str,
    cwd: &str,
    command: &str,
) -> Result<i32, String> {
    let spec = box_spec(runtime_key, image, cwd);
    ensure_box(runner, &spec)?;
    let argv = vec!["bash".to_string(), "-lc".to_string(), command.to_string()];
    runner.exec(&spec.name, &argv)
}

fn short_hash(input: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in input.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

pub struct RealLns;

impl LnsRunner for RealLns {
    fn inspect(&self, name: &str) -> Result<BoxState, String> {
        let out = Command::new("lns")
            .args(["sandbox", "inspect", name])
            .output()
            .map_err(|e| format!("failed to run `lns sandbox inspect`: {e}"))?;
        if !out.status.success() {
            return Ok(BoxState::NotFound);
        }
        let value: serde_json::Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("could not parse `lns sandbox inspect` output: {e}"))?;
        let state = value
            .get("status")
            .and_then(|s| s.get("state"))
            .and_then(serde_json::Value::as_str);
        Ok(match state {
            Some("running") => BoxState::Running,
            _ => BoxState::Stopped,
        })
    }

    fn boot(&self, spec: &BoxSpec) -> Result<(), String> {
        let out = Command::new("lns")
            .args([
                "run",
                "-d",
                "--name",
                &spec.name,
                "-v",
                &spec.mount,
                "-w",
                &spec.workdir,
            ])
            .arg(&spec.image)
            .args(["--", "sleep", "infinity"])
            .output()
            .map_err(|e| format!("failed to run `lns run`: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "could not start sandbox `{}`: {}",
                spec.name,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        Command::new("lns")
            .args(["sandbox", "rm", name])
            .output()
            .map_err(|e| format!("failed to run `lns sandbox rm`: {e}"))?;
        Ok(())
    }

    fn exec(&self, name: &str, argv: &[String]) -> Result<i32, String> {
        let status = Command::new("lns")
            .args(["sandbox", "exec", "-i", name, "--"])
            .args(argv)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("failed to run `lns sandbox exec`: {e}"))?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner {
        inspect: Result<BoxState, String>,
        exec_code: i32,
        calls: RefCell<Vec<String>>,
    }

    impl FakeRunner {
        fn new(inspect: Result<BoxState, String>) -> Self {
            Self {
                inspect,
                exec_code: 0,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl LnsRunner for FakeRunner {
        fn inspect(&self, name: &str) -> Result<BoxState, String> {
            self.calls.borrow_mut().push(format!("inspect:{name}"));
            self.inspect.clone()
        }
        fn boot(&self, spec: &BoxSpec) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("boot:{}", spec.name));
            Ok(())
        }
        fn remove(&self, name: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("remove:{name}"));
            Ok(())
        }
        fn exec(&self, name: &str, argv: &[String]) -> Result<i32, String> {
            self.calls
                .borrow_mut()
                .push(format!("exec:{name}:{}", argv.join(" ")));
            Ok(self.exec_code)
        }
    }

    #[test]
    fn box_name_is_deterministic_and_runtime_scoped() {
        assert_eq!(box_name("python", "/a"), box_name("python", "/a"));
        assert_ne!(box_name("python", "/a"), box_name("python", "/b"));
        assert_ne!(box_name("python", "/a"), box_name("node", "/a"));
        assert!(box_name("python", "/a").starts_with("cc-python-"));
    }

    #[test]
    fn box_spec_mounts_cwd_at_its_real_path() {
        let spec = box_spec("python", "python:3.12", "/Users/me/proj");
        assert_eq!(spec.mount, "/Users/me/proj:/Users/me/proj");
        assert_eq!(spec.workdir, "/Users/me/proj");
        assert_eq!(spec.image, "python:3.12");
    }

    #[test]
    fn running_box_is_reused_without_boot() {
        let runner = FakeRunner::new(Ok(BoxState::Running));
        let spec = box_spec("python", "python:3.12", "/p");
        ensure_box(&runner, &spec).unwrap();
        assert_eq!(
            runner.calls.borrow().clone(),
            vec![format!("inspect:{}", spec.name)]
        );
    }

    #[test]
    fn missing_box_is_booted() {
        let runner = FakeRunner::new(Ok(BoxState::NotFound));
        let spec = box_spec("node", "node:22", "/p");
        ensure_box(&runner, &spec).unwrap();
        let calls = runner.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                format!("inspect:{}", spec.name),
                format!("boot:{}", spec.name)
            ]
        );
    }

    #[test]
    fn stopped_box_is_removed_then_rebooted() {
        let runner = FakeRunner::new(Ok(BoxState::Stopped));
        let spec = box_spec("ruby", "ruby:3.3", "/p");
        ensure_box(&runner, &spec).unwrap();
        let calls = runner.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                format!("inspect:{}", spec.name),
                format!("remove:{}", spec.name),
                format!("boot:{}", spec.name),
            ]
        );
    }

    #[test]
    fn inspect_error_aborts_before_boot() {
        let runner = FakeRunner::new(Err("service down".to_string()));
        let spec = box_spec("python", "python:3.12", "/p");
        assert_eq!(ensure_box(&runner, &spec), Err("service down".to_string()));
        assert_eq!(runner.calls.borrow().len(), 1);
    }

    #[test]
    fn run_in_box_execs_command_under_login_shell_and_returns_code() {
        let mut runner = FakeRunner::new(Ok(BoxState::Running));
        runner.exec_code = 7;
        let code = run_in_box(&runner, "python", "python:3.12", "/p", "python app.py").unwrap();
        assert_eq!(code, 7);
        let name = box_name("python", "/p");
        assert!(runner
            .calls
            .borrow()
            .contains(&format!("exec:{name}:bash -lc python app.py")));
    }

    #[test]
    fn run_in_box_surfaces_ensure_failure_without_exec() {
        let runner = FakeRunner::new(Err("boom".to_string()));
        let err = run_in_box(&runner, "python", "python:3.12", "/p", "python app.py").unwrap_err();
        assert_eq!(err, "boom");
        assert!(runner
            .calls
            .borrow()
            .iter()
            .all(|c| !c.starts_with("exec:")));
    }
}
