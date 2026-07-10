use std::path::{Path, PathBuf};

mod real;
mod traits;

use traits::{CommandOutput, CommandRunner, Fs};

const LABEL: &str = "run.lns.service";
const SYSTEMD_UNIT_NAME: &str = "lns-service";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Platform {
    MacOs,
    Linux,
}

pub(super) struct AgentSpec {
    pub platform: Platform,
    pub label: String,
    pub agent_path: PathBuf,
    pub service_bin: PathBuf,
    pub uid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EnableOutcome {
    Registered,
    AlreadyRegistered,
    Degraded(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DisableOutcome {
    Unregistered,
    WasNotRegistered,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RestartOutcome {
    Relaunched,
    NotRegistered,
    Degraded(String),
}

fn platform_for_os(os: &str) -> Platform {
    match os {
        "macos" => Platform::MacOs,
        _ => Platform::Linux,
    }
}

fn current_platform() -> Platform {
    platform_for_os(std::env::consts::OS)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn render_plist(label: &str, program_args: &[String]) -> String {
    let mut lines = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".to_string(),
        "<plist version=\"1.0\">".to_string(),
        "<dict>".to_string(),
        "    <key>Label</key>".to_string(),
        format!("    <string>{}</string>", xml_escape(label)),
        "    <key>ProgramArguments</key>".to_string(),
        "    <array>".to_string(),
    ];
    for arg in program_args {
        lines.push(format!("        <string>{}</string>", xml_escape(arg)));
    }
    lines.extend(
        [
            "    </array>",
            "    <key>RunAtLoad</key>",
            "    <true/>",
            "    <key>KeepAlive</key>",
            "    <dict>",
            "        <key>Crashed</key>",
            "        <true/>",
            "    </dict>",
            "    <key>ProcessType</key>",
            "    <string>Interactive</string>",
            "</dict>",
            "</plist>",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn render_systemd_unit(service_bin: &str) -> Result<String, String> {
    if service_bin.contains('\n') {
        return Err(format!(
            "service binary path contains a newline and cannot be written to a line-oriented unit file: {service_bin:?}"
        ));
    }
    let lines = [
        "[Unit]".to_string(),
        "Description=Lens Sandbox background service".to_string(),
        "After=graphical-session.target".to_string(),
        String::new(),
        "[Service]".to_string(),
        "Type=simple".to_string(),
        format!("ExecStart={service_bin}"),
        "Restart=on-failure".to_string(),
        "RestartSec=2".to_string(),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
    ];
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

fn resolve_spec(platform: Platform, home: &Path, service_bin: &Path, uid: u32) -> AgentSpec {
    let agent_path = match platform {
        Platform::MacOs => home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist")),
        Platform::Linux => home
            .join(".config")
            .join("systemd")
            .join("user")
            .join(format!("{SYSTEMD_UNIT_NAME}.service")),
    };
    AgentSpec {
        platform,
        label: LABEL.to_string(),
        agent_path,
        service_bin: service_bin.to_path_buf(),
        uid,
    }
}

fn render_agent(spec: &AgentSpec) -> Result<String, String> {
    match spec.platform {
        Platform::MacOs => Ok(render_plist(
            &spec.label,
            &[spec.service_bin.to_string_lossy().into_owned()],
        )),
        Platform::Linux => render_systemd_unit(&spec.service_bin.to_string_lossy()),
    }
}

fn launchctl_already_registered(out: &CommandOutput) -> bool {
    out.status == 5
        || out.stderr.contains("service already bootstrapped")
        || out.stderr.contains("Bootstrap failed: 5")
}

fn launchctl_bootstrap_unavailable(out: &CommandOutput) -> bool {
    out.stderr.contains("Unrecognized subcommand")
        || out.stderr.contains("unknown subcommand")
        || out.stderr.contains("Unknown subcommand")
}

fn classify_runner_error(program: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!("{program} not found")
    } else {
        format!("{program} could not be run: {err}")
    }
}

fn degraded_reason(program: &str, out: &CommandOutput) -> String {
    let detail = out.stderr.lines().next().unwrap_or("").trim();
    if detail.is_empty() {
        format!("{program} exited with status {}", out.status)
    } else {
        format!("{program} failed: {detail}")
    }
}

async fn enable_with(fs: &impl Fs, runner: &impl CommandRunner, spec: &AgentSpec) -> EnableOutcome {
    let already_present = fs.exists(&spec.agent_path).await;
    let rendered = match render_agent(spec) {
        Ok(r) => r,
        Err(reason) => return EnableOutcome::Degraded(reason),
    };
    if let Some(parent) = spec.agent_path.parent()
        && let Err(e) = fs.create_dir_all(parent).await
    {
        return EnableOutcome::Degraded(format!("could not create {}: {e}", parent.display()));
    }
    if let Err(e) = fs.write(&spec.agent_path, &rendered).await {
        return EnableOutcome::Degraded(format!(
            "could not write {}: {e}",
            spec.agent_path.display()
        ));
    }
    match spec.platform {
        Platform::MacOs => enable_macos(runner, spec, already_present).await,
        Platform::Linux => enable_linux(runner, spec, already_present).await,
    }
}

async fn enable_macos(
    runner: &impl CommandRunner,
    spec: &AgentSpec,
    already_present: bool,
) -> EnableOutcome {
    let plist = spec.agent_path.to_string_lossy().into_owned();
    let bootstrap_args = vec![
        "bootstrap".to_string(),
        format!("gui/{}", spec.uid),
        plist.clone(),
    ];
    match runner.run("launchctl", &bootstrap_args).await {
        Ok(out) if out.status == 0 => EnableOutcome::Registered,
        Ok(out) if launchctl_already_registered(&out) => {
            if already_present {
                EnableOutcome::AlreadyRegistered
            } else {
                EnableOutcome::Registered
            }
        }
        Ok(out) if launchctl_bootstrap_unavailable(&out) => {
            launchctl_load_fallback(runner, &plist).await
        }
        Ok(out) => EnableOutcome::Degraded(degraded_reason("launchctl", &out)),
        Err(e) => EnableOutcome::Degraded(classify_runner_error("launchctl", &e)),
    }
}

async fn launchctl_load_fallback(runner: &impl CommandRunner, plist: &str) -> EnableOutcome {
    let load_args = vec!["load".to_string(), "-w".to_string(), plist.to_string()];
    match runner.run("launchctl", &load_args).await {
        Ok(out) if out.status == 0 => EnableOutcome::Registered,
        Ok(out) => EnableOutcome::Degraded(degraded_reason("launchctl", &out)),
        Err(e) => EnableOutcome::Degraded(classify_runner_error("launchctl", &e)),
    }
}

async fn enable_linux(
    runner: &impl CommandRunner,
    _spec: &AgentSpec,
    already_present: bool,
) -> EnableOutcome {
    let reload = vec!["--user".to_string(), "daemon-reload".to_string()];
    match runner.run("systemctl", &reload).await {
        Ok(out) if out.status != 0 => {
            return EnableOutcome::Degraded(degraded_reason("systemctl", &out));
        }
        Ok(_) => {}
        Err(e) => return EnableOutcome::Degraded(classify_runner_error("systemctl", &e)),
    }
    let enable = vec![
        "--user".to_string(),
        "enable".to_string(),
        "--now".to_string(),
        SYSTEMD_UNIT_NAME.to_string(),
    ];
    match runner.run("systemctl", &enable).await {
        Ok(out) if out.status == 0 => {
            if already_present {
                EnableOutcome::AlreadyRegistered
            } else {
                EnableOutcome::Registered
            }
        }
        Ok(out) => EnableOutcome::Degraded(degraded_reason("systemctl", &out)),
        Err(e) => EnableOutcome::Degraded(classify_runner_error("systemctl", &e)),
    }
}

async fn disable_with(
    fs: &impl Fs,
    runner: &impl CommandRunner,
    spec: &AgentSpec,
) -> DisableOutcome {
    let was_present = fs.exists(&spec.agent_path).await;
    match spec.platform {
        Platform::MacOs => disable_macos(runner, spec).await,
        Platform::Linux => disable_linux(runner).await,
    }
    let removed = fs.remove_file(&spec.agent_path).await.is_ok();
    if was_present || removed {
        DisableOutcome::Unregistered
    } else {
        DisableOutcome::WasNotRegistered
    }
}

async fn disable_macos(runner: &impl CommandRunner, spec: &AgentSpec) {
    let target = format!("gui/{}/{}", spec.uid, spec.label);
    let bootout = vec!["bootout".to_string(), target];
    let _ = runner.run("launchctl", &bootout).await;
}

async fn disable_linux(runner: &impl CommandRunner) {
    let disable = vec![
        "--user".to_string(),
        "disable".to_string(),
        "--now".to_string(),
        SYSTEMD_UNIT_NAME.to_string(),
    ];
    let _ = runner.run("systemctl", &disable).await;
    let reload = vec!["--user".to_string(), "daemon-reload".to_string()];
    let _ = runner.run("systemctl", &reload).await;
}

async fn restart_with(
    fs: &impl Fs,
    runner: &impl CommandRunner,
    spec: &AgentSpec,
) -> RestartOutcome {
    if !fs.exists(&spec.agent_path).await {
        return RestartOutcome::NotRegistered;
    }
    match spec.platform {
        Platform::MacOs => restart_macos(runner, spec).await,
        Platform::Linux => restart_linux(runner).await,
    }
}

async fn restart_macos(runner: &impl CommandRunner, spec: &AgentSpec) -> RestartOutcome {
    let target = format!("gui/{}/{}", spec.uid, spec.label);
    let _ = runner
        .run("launchctl", &["bootout".to_string(), target])
        .await;
    let plist = spec.agent_path.to_string_lossy().into_owned();
    let bootstrap = vec!["bootstrap".to_string(), format!("gui/{}", spec.uid), plist];
    match runner.run("launchctl", &bootstrap).await {
        Ok(out) if out.status == 0 => RestartOutcome::Relaunched,
        Ok(out) => RestartOutcome::Degraded(degraded_reason("launchctl", &out)),
        Err(e) => RestartOutcome::Degraded(classify_runner_error("launchctl", &e)),
    }
}

async fn restart_linux(runner: &impl CommandRunner) -> RestartOutcome {
    let restart = vec![
        "--user".to_string(),
        "restart".to_string(),
        SYSTEMD_UNIT_NAME.to_string(),
    ];
    match runner.run("systemctl", &restart).await {
        Ok(out) if out.status == 0 => RestartOutcome::Relaunched,
        Ok(out) => RestartOutcome::Degraded(degraded_reason("systemctl", &out)),
        Err(e) => RestartOutcome::Degraded(classify_runner_error("systemctl", &e)),
    }
}

fn home_dir() -> Result<PathBuf, String> {
    home_dir_with(|k| std::env::var_os(k))
}

fn home_dir_with(env_get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    match env_get("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => Err("HOME is not set; cannot locate the per-user login-agent directory".to_string()),
    }
}

fn current_uid() -> u32 {
    // SAFETY: getuid is always-succeeds and reentrant per POSIX.
    unsafe { libc::getuid() }
}

fn production_spec() -> Result<AgentSpec, String> {
    let home = home_dir()?;
    let service_bin = super::find_service_binary();
    Ok(resolve_spec(
        current_platform(),
        &home,
        &service_bin,
        current_uid(),
    ))
}

pub(super) use real::{disable, enable, restart};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeState {
        files: std::collections::HashMap<PathBuf, String>,
        fail_write: Option<io::Error>,
        fail_create_dir: Option<io::Error>,
        fail_remove: Option<io::Error>,
    }

    #[derive(Clone, Default)]
    struct FakeFs {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeFs {
        fn put(&self, p: impl Into<PathBuf>) {
            self.state
                .lock()
                .unwrap()
                .files
                .insert(p.into(), String::new());
        }
        fn contents(&self, p: &Path) -> Option<String> {
            self.state.lock().unwrap().files.get(p).cloned()
        }
        fn fail_next_write(&self, e: io::Error) {
            self.state.lock().unwrap().fail_write = Some(e);
        }
        fn fail_next_create_dir(&self, e: io::Error) {
            self.state.lock().unwrap().fail_create_dir = Some(e);
        }
        fn fail_next_remove(&self, e: io::Error) {
            self.state.lock().unwrap().fail_remove = Some(e);
        }
    }

    impl Fs for FakeFs {
        async fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_write.take() {
                return Err(e);
            }
            s.files.insert(path.to_path_buf(), contents.to_string());
            Ok(())
        }
        async fn remove_file(&self, path: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_remove.take() {
                return Err(e);
            }
            if s.files.remove(path).is_some() {
                Ok(())
            } else {
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
        }
        async fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_create_dir.take() {
                return Err(e);
            }
            Ok(())
        }
        async fn exists(&self, path: &Path) -> bool {
            self.state.lock().unwrap().files.contains_key(path)
        }
    }

    #[derive(Default)]
    struct RunnerState {
        plan: VecDeque<io::Result<CommandOutput>>,
        calls: Vec<(String, Vec<String>)>,
    }

    #[derive(Clone, Default)]
    struct FakeRunner {
        state: Arc<Mutex<RunnerState>>,
    }

    impl FakeRunner {
        fn push_ok(&self, status: i32, stderr: &str) {
            self.state.lock().unwrap().plan.push_back(Ok(CommandOutput {
                status,
                stderr: stderr.to_string(),
            }));
        }
        fn push_err(&self, e: io::Error) {
            self.state.lock().unwrap().plan.push_back(Err(e));
        }
        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.state.lock().unwrap().calls.clone()
        }
    }

    impl CommandRunner for FakeRunner {
        async fn run(&self, program: &str, args: &[String]) -> io::Result<CommandOutput> {
            let mut s = self.state.lock().unwrap();
            s.calls.push((program.to_string(), args.to_vec()));
            s.plan
                .pop_front()
                .expect("FakeRunner: run called more times than planned")
        }
    }

    fn macos_spec() -> AgentSpec {
        resolve_spec(
            Platform::MacOs,
            Path::new("/Users/alice"),
            Path::new("/Users/alice/.local/bin/lns-service"),
            501,
        )
    }

    fn linux_spec() -> AgentSpec {
        resolve_spec(
            Platform::Linux,
            Path::new("/home/alice"),
            Path::new("/home/alice/.local/bin/lns-service"),
            1000,
        )
    }

    #[track_caller]
    fn assert_degraded(outcome: &EnableOutcome, needle: &str) {
        let rendered = format!("{outcome:?}");
        assert!(
            rendered.starts_with("Degraded("),
            "expected a Degraded outcome, got {rendered}"
        );
        assert!(
            rendered.contains(needle),
            "expected degraded reason to mention {needle:?}, got {rendered}"
        );
    }

    #[test]
    fn render_plist_embeds_label_program_args_runatload_and_crashed_only_keepalive() {
        let plist = render_plist(LABEL, &["/Users/alice/.local/bin/lns-service".to_string()]);
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("<string>run.lns.service</string>"));
        assert!(plist.contains("<string>/Users/alice/.local/bin/lns-service</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(
            plist.contains("<key>KeepAlive</key>\n    <dict>\n        <key>Crashed</key>\n        <true/>\n    </dict>"),
            "KeepAlive must be a Crashed-only dict: {plist}"
        );
        assert!(
            !plist.contains("<key>KeepAlive</key>\n    <true/>"),
            "must not relaunch on clean exit: {plist}"
        );
        assert!(plist.contains("<key>ProcessType</key>\n    <string>Interactive</string>"));
    }

    #[test]
    fn render_plist_xml_escapes_special_chars_in_paths() {
        let plist = render_plist(LABEL, &["/Users/a b/R&D/lns-service".to_string()]);
        assert!(
            plist.contains("<string>/Users/a b/R&amp;D/lns-service</string>"),
            "ampersand must be escaped and space preserved: {plist}"
        );
        assert!(!plist.contains("R&D"), "raw ampersand leaked: {plist}");
    }

    #[test]
    fn xml_escape_covers_all_five_entities() {
        assert_eq!(xml_escape("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
        assert_eq!(xml_escape("plain"), "plain");
    }

    #[test]
    fn render_systemd_unit_has_restart_on_failure_default_target_and_execstart() {
        let unit = render_systemd_unit("/home/alice/.local/bin/lns-service").unwrap();
        assert!(unit.contains("Restart=on-failure"), "{unit}");
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert!(
            unit.contains("ExecStart=/home/alice/.local/bin/lns-service"),
            "{unit}"
        );
        assert!(unit.contains("Type=simple"), "{unit}");
        assert!(!unit.contains("Restart=always"), "{unit}");
    }

    #[test]
    fn render_systemd_unit_rejects_a_newline_in_the_path() {
        let err = render_systemd_unit("/home/a\nb/lns-service").unwrap_err();
        assert!(err.contains("newline"), "got: {err}");
    }

    #[test]
    fn resolve_spec_macos_targets_launchagents_plist() {
        let spec = macos_spec();
        assert_eq!(spec.platform, Platform::MacOs);
        assert_eq!(
            spec.agent_path,
            PathBuf::from("/Users/alice/Library/LaunchAgents/run.lns.service.plist")
        );
        assert_eq!(spec.label, "run.lns.service");
    }

    #[test]
    fn resolve_spec_linux_targets_user_systemd_unit() {
        let spec = linux_spec();
        assert_eq!(spec.platform, Platform::Linux);
        assert_eq!(
            spec.agent_path,
            PathBuf::from("/home/alice/.config/systemd/user/lns-service.service")
        );
    }

    #[tokio::test]
    async fn enable_macos_happy_path_writes_plist_and_runs_bootstrap() {
        let fs = FakeFs::default();
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        let spec = macos_spec();

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::Registered);
        assert!(
            fs.contents(&spec.agent_path).is_some(),
            "plist must be written"
        );
        assert_eq!(
            runner.calls(),
            vec![(
                "launchctl".to_string(),
                vec![
                    "bootstrap".to_string(),
                    "gui/501".to_string(),
                    "/Users/alice/Library/LaunchAgents/run.lns.service.plist".to_string(),
                ]
            )]
        );
    }

    #[tokio::test]
    async fn enable_linux_happy_path_runs_daemon_reload_then_enable_now() {
        let fs = FakeFs::default();
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(0, "Created symlink ...");
        let spec = linux_spec();

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::Registered);
        let calls = runner.calls();
        assert_eq!(calls[0].0, "systemctl");
        assert_eq!(calls[0].1, vec!["--user", "daemon-reload"]);
        assert_eq!(calls[1].1, vec!["--user", "enable", "--now", "lns-service"]);
    }

    #[tokio::test]
    async fn enable_macos_idempotent_when_file_present_and_already_bootstrapped() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(5, "Bootstrap failed: 5: service already bootstrapped");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::AlreadyRegistered);
    }

    #[tokio::test]
    async fn enable_macos_already_bootstrapped_but_fresh_file_is_registered() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(5, "service already bootstrapped");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::Registered);
    }

    #[tokio::test]
    async fn enable_linux_idempotent_when_unit_already_present() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(0, "");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::AlreadyRegistered);
    }

    #[tokio::test]
    async fn enable_macos_falls_back_to_load_when_bootstrap_unavailable() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(64, "Unrecognized subcommand: bootstrap");
        runner.push_ok(0, "");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, EnableOutcome::Registered);
        let calls = runner.calls();
        assert_eq!(
            calls[1].1,
            vec![
                "load",
                "-w",
                "/Users/alice/Library/LaunchAgents/run.lns.service.plist"
            ]
        );
    }

    #[tokio::test]
    async fn enable_macos_load_fallback_can_still_degrade() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(64, "unknown subcommand");
        runner.push_ok(1, "Load failed: 5: input/output error");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert!(matches!(outcome, EnableOutcome::Degraded(_)), "{outcome:?}");
    }

    #[tokio::test]
    async fn enable_macos_load_fallback_runner_error_degrades() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(64, "Unknown subcommand");
        runner.push_err(io::Error::from(io::ErrorKind::NotFound));

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "not found");
    }

    #[tokio::test]
    async fn enable_degrades_when_runner_binary_not_found_but_file_is_written() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_err(io::Error::from(io::ErrorKind::NotFound));

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "launchctl not found");
        assert!(
            fs.contents(&spec.agent_path).is_some(),
            "plist still written so next login can pick it up"
        );
    }

    #[tokio::test]
    async fn enable_macos_degrades_on_no_gui_session() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(125, "Bootstrap failed: 125: Could not find domain for");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "Bootstrap failed: 125");
    }

    #[tokio::test]
    async fn enable_linux_degrades_on_no_dbus_bus_at_reload() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        let runner = FakeRunner::default();
        runner.push_ok(1, "Failed to connect to bus: No medium found");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "Failed to connect to bus");
        assert_eq!(
            runner.calls().len(),
            1,
            "must not attempt enable after reload fails"
        );
    }

    #[tokio::test]
    async fn enable_linux_reload_runner_error_degrades() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        let runner = FakeRunner::default();
        runner.push_err(io::Error::from(io::ErrorKind::NotFound));

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "systemctl not found");
    }

    #[tokio::test]
    async fn enable_linux_enable_step_degrades_on_failure() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(1, "Failed to connect to user scope bus");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert!(matches!(outcome, EnableOutcome::Degraded(_)), "{outcome:?}");
    }

    #[tokio::test]
    async fn enable_linux_enable_step_runner_error_degrades() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_err(io::Error::other("broken pipe"));

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "could not be run");
    }

    #[tokio::test]
    async fn enable_degrades_with_empty_stderr_reports_status() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(99, "");

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "status 99");
    }

    #[tokio::test]
    async fn enable_degrades_when_write_denied_and_runner_never_called() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.fail_next_write(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
        let runner = FakeRunner::default();

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "could not write");
        assert_degraded(&outcome, "run.lns.service.plist");
        assert!(
            runner.calls().is_empty(),
            "write failure must short-circuit"
        );
    }

    #[tokio::test]
    async fn enable_degrades_when_create_dir_denied_and_nothing_else_runs() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.fail_next_create_dir(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        let runner = FakeRunner::default();

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "could not create");
        assert!(
            fs.contents(&spec.agent_path).is_none(),
            "no write after mkdir failure"
        );
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn enable_degrades_when_systemd_path_has_newline() {
        let fs = FakeFs::default();
        let spec = resolve_spec(
            Platform::Linux,
            Path::new("/home/alice"),
            Path::new("/home/a\nb/lns-service"),
            1000,
        );
        let runner = FakeRunner::default();

        let outcome = enable_with(&fs, &runner, &spec).await;
        assert_degraded(&outcome, "newline");
        assert!(runner.calls().is_empty());
        assert!(fs.contents(&spec.agent_path).is_none());
    }

    #[tokio::test]
    async fn disable_macos_runs_bootout_then_removes_plist() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");

        let outcome = disable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, DisableOutcome::Unregistered);
        assert!(fs.contents(&spec.agent_path).is_none(), "plist removed");
        let calls = runner.calls();
        assert_eq!(calls[0].0, "launchctl");
        assert_eq!(calls[0].1, vec!["bootout", "gui/501/run.lns.service"]);
    }

    #[tokio::test]
    async fn disable_linux_runs_disable_now_then_reload_then_removes_unit() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(0, "");

        let outcome = disable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, DisableOutcome::Unregistered);
        let calls = runner.calls();
        assert_eq!(
            calls[0].1,
            vec!["--user", "disable", "--now", "lns-service"]
        );
        assert_eq!(calls[1].1, vec!["--user", "daemon-reload"]);
        assert!(fs.contents(&spec.agent_path).is_none());
    }

    #[tokio::test]
    async fn disable_macos_treats_not_loaded_and_absent_file_as_was_not_registered() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        let runner = FakeRunner::default();
        runner.push_ok(3, "No such process");

        let outcome = disable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, DisableOutcome::WasNotRegistered);
    }

    #[tokio::test]
    async fn disable_reports_unregistered_when_file_was_present_even_if_remove_fails() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        fs.fail_next_remove(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        let runner = FakeRunner::default();
        runner.push_ok(0, "");

        let outcome = disable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, DisableOutcome::Unregistered);
    }

    #[tokio::test]
    async fn disable_linux_treats_not_loaded_and_absent_file_as_was_not_registered() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        let runner = FakeRunner::default();
        runner.push_ok(
            1,
            "Unit lns-service.service does not exist, proceeding anyway.",
        );
        runner.push_ok(0, "");

        let outcome = disable_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, DisableOutcome::WasNotRegistered);
    }

    #[tokio::test]
    async fn restart_returns_not_registered_when_no_agent_file() {
        let fs = FakeFs::default();
        let runner = FakeRunner::default();
        let spec = macos_spec();

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, RestartOutcome::NotRegistered);
        assert!(
            runner.calls().is_empty(),
            "an unregistered agent must not touch launchctl"
        );
    }

    #[tokio::test]
    async fn restart_macos_boots_out_then_bootstraps_the_plist() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(0, "");

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, RestartOutcome::Relaunched);
        let calls = runner.calls();
        assert_eq!(calls[0].1, vec!["bootout", "gui/501/run.lns.service"]);
        assert_eq!(
            calls[1].1,
            vec![
                "bootstrap",
                "gui/501",
                "/Users/alice/Library/LaunchAgents/run.lns.service.plist"
            ]
        );
    }

    #[tokio::test]
    async fn restart_macos_degrades_when_bootstrap_fails() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_ok(125, "Bootstrap failed: 125: Could not find domain for");

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert!(
            matches!(&outcome, RestartOutcome::Degraded(r) if r.contains("Bootstrap failed: 125")),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn restart_macos_degrades_when_launchctl_missing() {
        let fs = FakeFs::default();
        let spec = macos_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");
        runner.push_err(io::Error::from(io::ErrorKind::NotFound));

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert!(
            matches!(&outcome, RestartOutcome::Degraded(r) if r.contains("launchctl not found")),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn restart_linux_restarts_the_user_unit() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(0, "");

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert_eq!(outcome, RestartOutcome::Relaunched);
        assert_eq!(
            runner.calls()[0].1,
            vec!["--user", "restart", "lns-service"]
        );
    }

    #[tokio::test]
    async fn restart_linux_degrades_on_failure() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_ok(1, "Failed to connect to bus: No medium found");

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert!(
            matches!(&outcome, RestartOutcome::Degraded(r) if r.contains("Failed to connect to bus")),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn restart_linux_degrades_when_systemctl_missing() {
        let fs = FakeFs::default();
        let spec = linux_spec();
        fs.put(&spec.agent_path);
        let runner = FakeRunner::default();
        runner.push_err(io::Error::from(io::ErrorKind::NotFound));

        let outcome = restart_with(&fs, &runner, &spec).await;
        assert!(
            matches!(&outcome, RestartOutcome::Degraded(r) if r.contains("systemctl not found")),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn restart_production_entry_not_registered_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        assert_eq!(restart().await, RestartOutcome::NotRegistered);
    }

    #[test]
    fn platform_for_os_maps_macos_and_treats_everything_else_as_linux() {
        assert_eq!(platform_for_os("macos"), Platform::MacOs);
        assert_eq!(platform_for_os("linux"), Platform::Linux);
        assert_eq!(platform_for_os("freebsd"), Platform::Linux);
    }

    #[test]
    fn current_platform_matches_the_host() {
        assert_eq!(current_platform(), platform_for_os(std::env::consts::OS));
    }

    #[test]
    fn current_uid_is_deterministic() {
        assert_eq!(current_uid(), current_uid());
    }

    #[test]
    fn home_dir_with_reads_home_env() {
        let p = home_dir_with(|k| (k == "HOME").then(|| std::ffi::OsString::from("/home/zed")))
            .expect("home resolves");
        assert_eq!(p, PathBuf::from("/home/zed"));
    }

    #[test]
    fn home_dir_with_errors_when_home_unset_or_empty() {
        let unset = home_dir_with(|_| None).unwrap_err();
        assert!(unset.contains("HOME is not set"), "{unset}");
        let empty = home_dir_with(|_| Some(std::ffi::OsString::new())).unwrap_err();
        assert!(empty.contains("HOME is not set"), "{empty}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn home_dir_reads_process_home() {
        let _g = crate::test_env::EnvScope::set("HOME", "/home/from-env");
        assert_eq!(home_dir().unwrap(), PathBuf::from("/home/from-env"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn production_spec_resolves_for_the_host_under_a_set_home() {
        let _g = crate::test_env::EnvScope::set("HOME", "/home/spec-test");
        let spec = production_spec().expect("spec resolves with HOME set");
        assert_eq!(spec.platform, current_platform());
        assert!(spec.agent_path.starts_with("/home/spec-test"));
        assert_eq!(spec.uid, current_uid());
    }

    #[test]
    #[serial_test::serial(env)]
    fn production_spec_errors_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        assert!(production_spec().is_err());
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn enable_production_entry_degrades_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        let outcome = enable().await;
        assert!(matches!(outcome, EnableOutcome::Degraded(_)), "{outcome:?}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn disable_production_entry_is_was_not_registered_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        let outcome = disable().await;
        assert_eq!(outcome, DisableOutcome::WasNotRegistered);
    }
}
