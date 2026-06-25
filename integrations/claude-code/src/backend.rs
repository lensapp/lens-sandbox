use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

pub trait Backend {
    fn run_in_sandbox(
        &self,
        runtime_key: &str,
        image: &str,
        cwd: &str,
        command: &str,
    ) -> Result<i32, String>;
}

pub fn box_name(runtime_key: &str, cwd: &str) -> String {
    format!("cc-{runtime_key}-{}", short_hash(cwd))
}

pub fn shell_argv(command: &str) -> Vec<String> {
    vec!["bash".to_string(), "-lc".to_string(), command.to_string()]
}

fn short_hash(input: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in input.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub name: String,
    pub image: String,
    pub mount: String,
    pub workdir: String,
    pub argv: Vec<String>,
}

pub trait EphemeralLns {
    fn run_filtered(&self, spec: &RunSpec) -> Result<i32, String>;
}

pub struct EphemeralBackend<C: EphemeralLns> {
    lns: C,
}

impl<C: EphemeralLns> EphemeralBackend<C> {
    pub fn new(lns: C) -> Self {
        Self { lns }
    }
}

impl<C: EphemeralLns> Backend for EphemeralBackend<C> {
    fn run_in_sandbox(
        &self,
        runtime_key: &str,
        image: &str,
        cwd: &str,
        command: &str,
    ) -> Result<i32, String> {
        let spec = RunSpec {
            name: box_name(runtime_key, cwd),
            image: image.to_string(),
            mount: format!("{cwd}:{cwd}"),
            workdir: cwd.to_string(),
            argv: shell_argv(command),
        };
        self.lns.run_filtered(&spec)
    }
}

pub struct RealEphemeral;

impl EphemeralLns for RealEphemeral {
    fn run_filtered(&self, spec: &RunSpec) -> Result<i32, String> {
        let _ = Command::new("lns")
            .args(["sandbox", "rm", &spec.name])
            .output();
        let devnull =
            std::fs::File::open("/dev/null").map_err(|e| format!("cannot open /dev/null: {e}"))?;
        let mut child = Command::new("lns")
            .args([
                "run",
                "--name",
                &spec.name,
                "-v",
                &spec.mount,
                "-w",
                &spec.workdir,
            ])
            .arg(&spec.image)
            .arg("--")
            .args(&spec.argv)
            .stdin(Stdio::from(devnull))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to run `lns run`: {e}"))?;

        if let Some(out) = child.stdout.take() {
            let mut stdout = std::io::stdout().lock();
            for line in BufReader::new(out).lines() {
                let line = line.map_err(|e| format!("reading sandbox output: {e}"))?;
                if !is_supervisor_noise(&line) {
                    let _ = writeln!(stdout, "{line}");
                }
            }
        }
        let status = child
            .wait()
            .map_err(|e| format!("waiting on `lns run`: {e}"))?;
        let _ = Command::new("lns")
            .args(["sandbox", "rm", &spec.name])
            .output();
        Ok(status.code().unwrap_or(1))
    }
}

/// Drops the guest supervisor's tracing and `[agent]` markers that a PTY-mode `lns run` merges onto
/// stdout; remove this filter once lensapp/lens-sandbox#94 lands a non-interactive (no-PTY) mode.
pub fn is_supervisor_noise(line: &str) -> bool {
    let stripped = strip_ansi(line);
    let trimmed = stripped.trim_start();
    if trimmed.starts_with("[agent]") {
        return true;
    }
    iso_timestamp_prefixed(trimmed)
        && [" TRACE ", " DEBUG ", " INFO ", " WARN ", " ERROR "]
            .iter()
            .any(|level| trimmed.contains(level))
}

fn iso_timestamp_prefixed(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 11
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[7] == b'-'
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
        && b[10] == b'T'
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeEphemeral {
        code: i32,
        seen: RefCell<Option<RunSpec>>,
    }

    impl EphemeralLns for FakeEphemeral {
        fn run_filtered(&self, spec: &RunSpec) -> Result<i32, String> {
            *self.seen.borrow_mut() = Some(spec.clone());
            Ok(self.code)
        }
    }

    #[test]
    fn box_name_is_deterministic_and_scoped() {
        assert_eq!(box_name("python", "/a"), box_name("python", "/a"));
        assert_ne!(box_name("python", "/a"), box_name("python", "/b"));
        assert_ne!(box_name("python", "/a"), box_name("node", "/a"));
        assert!(box_name("python", "/a").starts_with("cc-python-"));
    }

    #[test]
    fn shell_argv_wraps_in_login_shell() {
        assert_eq!(
            shell_argv("python app.py"),
            vec![
                "bash".to_string(),
                "-lc".to_string(),
                "python app.py".to_string()
            ]
        );
    }

    #[test]
    fn ephemeral_backend_builds_real_path_mount_and_returns_code() {
        let fake = FakeEphemeral {
            code: 5,
            seen: RefCell::new(None),
        };
        let backend = EphemeralBackend::new(fake);
        let code = backend
            .run_in_sandbox("python", "python:3.12", "/Users/me/proj", "python app.py")
            .unwrap();
        assert_eq!(code, 5);
        let spec = backend.lns.seen.borrow().clone().unwrap();
        assert_eq!(spec.name, box_name("python", "/Users/me/proj"));
        assert_eq!(spec.image, "python:3.12");
        assert_eq!(spec.mount, "/Users/me/proj:/Users/me/proj");
        assert_eq!(spec.workdir, "/Users/me/proj");
        assert_eq!(spec.argv, shell_argv("python app.py"));
    }

    #[test]
    fn supervisor_tracing_lines_are_noise() {
        let line = "\u{1b}[2m2026-06-25T07:02:28.915029Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m starting agent sandbox";
        assert!(is_supervisor_noise(line));
        assert!(is_supervisor_noise(
            "2026-06-25T07:02:28.927357Z  WARN policy missing"
        ));
        assert!(is_supervisor_noise(
            "2026-06-25T07:02:28.971494Z  INFO agent process exited"
        ));
    }

    #[test]
    fn agent_markers_are_noise() {
        assert!(is_supervisor_noise(
            "\u{1b}[36m[agent]\u{1b}[0m starting: python app.py"
        ));
        assert!(is_supervisor_noise("[agent] exited with code 7"));
    }

    #[test]
    fn real_command_output_is_kept() {
        assert!(!is_supervisor_noise("STDOUT_LINE"));
        assert!(!is_supervisor_noise("marker-content-xyz"));
        assert!(!is_supervisor_noise("Traceback (most recent call last):"));
        assert!(!is_supervisor_noise("  File \"app.py\", line 3"));
    }

    #[test]
    fn timestamp_without_level_is_kept() {
        assert!(!is_supervisor_noise(
            "2026-06-25T07:02:28Z released version 1.0"
        ));
    }

    #[test]
    fn level_word_without_timestamp_is_kept() {
        assert!(!is_supervisor_noise("the INFO command printed this"));
    }

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\u{1b}[32mgreen\u{1b}[0m text"), "green text");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
