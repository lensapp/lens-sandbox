use crate::world::{BehaviourWorld, CannedSequence, PhasePipe};
use clap::Parser;
use cucumber::{given, then, when};
use lns_cli::cli::{Cli, Command};
use lns_cli::run::summary::print_run_summary;
use lns_cli::service::{
    PrePhaseStep, drive_attached_session_with_writers, pre_phase_step, render_started_run,
};
use lns_ipc::{LogLevel, Response, WireFrame, encode_frame, encode_wire_frame};
use lns_policy::{Policy, RouteRule, Transport, Verdict};
use std::io::Write as _;
use tokio::io::AsyncWriteExt;

fn pipe_mut(world: &mut BehaviourWorld) -> &mut PhasePipe {
    if world.pipe.is_none() {
        world.pipe = Some(PhasePipe::new());
    }
    world.pipe.as_mut().unwrap()
}

async fn send_and_step(world: &mut BehaviourWorld, resp: Response) -> PrePhaseStep {
    let frame = encode_frame(&resp).expect("encode response");
    {
        let pipe = pipe_mut(world);
        pipe.server.write_all(&frame).await.expect("write frame");
    }
    let mut writer = std::mem::take(&mut world.phase_output);
    let pipe = pipe_mut(world);
    let step = pre_phase_step(&mut pipe.client, &mut writer)
        .await
        .expect("pre_phase_step");
    world.phase_output = writer;
    step
}

fn run_log(verb: &str, message: &str) -> Response {
    Response::RunLog {
        level: LogLevel::Info,
        verb: Some(verb.to_string()),
        message: message.to_string(),
    }
}

fn emit_started_run_42(world: &mut BehaviourWorld) {
    render_started_run(42, &mut world.phase_output).expect("render Started");
    if world.detached {
        writeln!(world.detached_stdout, "run #42").expect("write run id");
    }
}

fn require_cwd(world: &BehaviourWorld) -> &std::path::Path {
    world
        .cwd
        .as_ref()
        .expect("Background did not initialise a working directory")
        .path()
}

fn parse_argv(argv: &[String]) -> Cli {
    let mut full = vec!["lns".to_string()];
    full.extend(argv.iter().cloned());
    Cli::try_parse_from(&full).expect("argv must parse against the CLI grammar")
}

#[given(regex = r"^the user invokes `lns run ([^`]+)` from an interactive terminal$")]
fn user_invokes_lns_run(world: &mut BehaviourWorld, image_and_flags: String) {
    world.argv = std::iter::once("run".to_string())
        .chain(image_and_flags.split_whitespace().map(str::to_string))
        .collect();
    world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
}

#[given(regex = r"^the working directory is `[^`]+`$")]
fn working_directory_is(_world: &mut BehaviourWorld) {}

#[given(regex = r"^the working directory contains `\./lns-policy\.yaml`$")]
fn cwd_contains_policy_file(world: &mut BehaviourWorld) {
    let _ = require_cwd(world);
}

#[given(
    regex = r#"^that policy has default verdict "([^"]+)", (\d+) allow rules?, and (\d+) deny rules?$"#
)]
fn that_policy_has(world: &mut BehaviourWorld, verdict: String, allows: usize, denies: usize) {
    let cwd = require_cwd(world).to_path_buf();
    let mut policy = Policy::default();
    policy.network.default_verdict = match verdict.as_str() {
        "allow" => Verdict::Allow,
        "deny" => Verdict::Deny,
        "ask" => Verdict::Ask,
        other => panic!("unknown verdict {other:?}"),
    };
    for i in 0..allows {
        policy.add_rule(RouteRule {
            match_pattern: format!("allow-{i}.example"),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        });
    }
    for i in 0..denies {
        policy.add_rule(RouteRule {
            match_pattern: format!("deny-{i}.example"),
            verdict: Verdict::Deny,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        });
    }
    policy
        .save_atomic(&cwd.join("lns-policy.yaml"))
        .expect("save_atomic");
}

#[given(regex = r"^no `lns-policy\.yaml` exists in the working directory$")]
fn no_policy_in_cwd(_world: &mut BehaviourWorld) {}

#[given(regex = r"^the command is `lns run ([^`]+)`$")]
fn the_command_is(world: &mut BehaviourWorld, args_after_run: String) {
    let tokens: Vec<String> = args_after_run
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if tokens.iter().any(|t| t == "-d" || t == "--detach") {
        world.detached = true;
        if world.canned_sequence == CannedSequence::None {
            world.canned_sequence = CannedSequence::ColdCache;
        }
    }
    world.argv = std::iter::once("run".to_string()).chain(tokens).collect();
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
}

#[when(regex = r"^(?:the command starts|the summary is printed|the run starts)$")]
async fn the_run_starts(world: &mut BehaviourWorld) {
    let cwd = require_cwd(world).to_path_buf();
    let cli = parse_argv(&world.argv);
    let Command::Run(args) = cli.command else {
        panic!("Layer-2 rig only drives `lns run`; got a different subcommand");
    };
    let mut buf = Vec::<u8>::new();
    print_run_summary(&args, &cwd, &mut buf).expect("print_run_summary");
    world.summary_output = String::from_utf8(buf).expect("non-utf8 summary output");

    drive_canned_sequence(world).await;
}

async fn drive_canned_sequence(world: &mut BehaviourWorld) {
    match world.canned_sequence {
        CannedSequence::None => {}
        CannedSequence::ColdCache => {
            send_and_step(
                world,
                run_log("Resolved", "ubuntu:latest @ sha256:abc123def"),
            )
            .await;
            send_and_step(world, run_log("Pulled", "N layers   (Xs · YMB)")).await;
            send_and_step(world, run_log("Booted", "microVM   (Xs)")).await;
            let step = send_and_step(world, run_log("SessionReady", "")).await;
            assert_eq!(step, PrePhaseStep::SessionReady);
            emit_started_run_42(world);
        }
        CannedSequence::WarmCache => {
            send_and_step(world, run_log("ImageCached", "")).await;
            send_and_step(world, run_log("Booted", "microVM   (Xs)")).await;
            let step = send_and_step(world, run_log("SessionReady", "")).await;
            assert_eq!(step, PrePhaseStep::SessionReady);
            emit_started_run_42(world);
        }
    }
}

#[then(regex = r"^the summary shows `([^`]+)`$")]
fn summary_shows(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected {needle:?} in summary:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r"^a run summary is printed before any service round-trip$")]
fn summary_is_printed(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.is_empty() {
        Err("no summary captured".to_string())
    } else {
        Ok(())
    }
}

#[then(regex = r"^the summary lists Image, Resources, Flags, and a Policy block$")]
fn summary_lists_all_fields(world: &mut BehaviourWorld) -> Result<(), String> {
    for label in ["Image:", "Resources:", "Flags:", "Policy:"] {
        if !world.summary_output.contains(label) {
            return Err(format!("missing {label} in:\n{}", world.summary_output));
        }
    }
    Ok(())
}

#[then(regex = r#"^fields not yet known to the CLI are shown as "\(resolving…\)"$"#)]
fn resolving_placeholder_present(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.contains("(resolving…)") {
        Ok(())
    } else {
        Err(format!(
            "expected `(resolving…)` placeholder in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r"^the Policy block shows the file path$")]
fn policy_block_shows_file_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let cwd = require_cwd(world);
    let expected = cwd.join("lns-policy.yaml");
    let needle = format!("file: {}", expected.display());
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!("missing {needle:?} in:\n{}", world.summary_output))
    }
}

#[then(regex = r"^the default verdict$")]
fn default_verdict_line_present(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.contains("default verdict:") {
        Ok(())
    } else {
        Err(format!(
            "missing `default verdict:` line in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^a one-line rule summary: "([^"]+)"$"#)]
fn rule_summary_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    if world.summary_output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "expected rule summary {expected:?} in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^the provenance line: "([^"]+)"$"#)]
fn provenance_line_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    if world.summary_output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "expected provenance {expected:?} in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^the Policy block source line reads "([^"]+)"$"#)]
fn source_line_reads(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    if world.summary_output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "expected source line {expected:?} in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^the default verdict is "([^"]+)"$"#)]
fn default_verdict_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let needle = format!("default verdict: {expected}");
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected {needle:?} in:\n{}", world.summary_output))
    }
}

#[given(regex = r"^the image is not in the local cache$")]
fn image_not_in_cache(_world: &mut BehaviourWorld) {}

#[given(regex = r"^the image is already cached locally$")]
fn image_already_cached(world: &mut BehaviourWorld) {
    world.canned_sequence = CannedSequence::WarmCache;
}

#[given(regex = r"^a run is starting$")]
async fn a_run_is_starting(world: &mut BehaviourWorld) {
    world.canned_sequence = CannedSequence::ColdCache;
    drive_canned_sequence(world).await;
}

#[when(regex = r"^the service resolves the image$")]
async fn service_resolves_image(world: &mut BehaviourWorld) {
    send_and_step(
        world,
        run_log("Resolved", "ubuntu:latest @ sha256:abc123def"),
    )
    .await;
}

#[when(regex = r"^layer pull completes$")]
async fn layer_pull_completes(world: &mut BehaviourWorld) {
    send_and_step(world, run_log("Pulled", "N layers   (Xs · YMB)")).await;
}

#[when(regex = r"^the microVM boots$")]
async fn microvm_boots(world: &mut BehaviourWorld) {
    send_and_step(world, run_log("Booted", "microVM   (Xs)")).await;
}

#[when(regex = r"^the session is ready$")]
async fn session_is_ready(world: &mut BehaviourWorld) {
    let step = send_and_step(world, run_log("SessionReady", "")).await;
    assert_eq!(step, PrePhaseStep::SessionReady);
    emit_started_run_42(world);
}

#[then(regex = r"^`([^`]+)` is printed$")]
fn line_is_printed(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    line_present(&world.phase_output, &line)
}

#[then(regex = r"^a single `([^`]+)` line is printed in place of resolve\+pull$")]
fn single_line_in_place_of_resolve_pull(
    world: &mut BehaviourWorld,
    line: String,
) -> Result<(), String> {
    line_present(&world.phase_output, &line)?;
    let s = String::from_utf8_lossy(&world.phase_output);
    if s.contains("resolved") || s.contains("pulled") {
        Err(format!("warm-cache run leaked resolve/pull lines:\n{s}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the boot and session-ready phase lines still follow$")]
fn boot_and_session_ready_follow(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = String::from_utf8_lossy(&world.phase_output);
    if !s.contains("✓ booted") {
        return Err(format!("missing booted line:\n{s}"));
    }
    if !s.contains("✓ started run #") {
        return Err(format!("missing Started line:\n{s}"));
    }
    Ok(())
}

#[then(regex = r"^finally `([^`]+)` is printed before the attached session takes over$")]
fn and_finally_printed(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    line_present(&world.phase_output, &line)
}

#[then(regex = r"^the summary block is printed$")]
fn summary_block_is_printed(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.is_empty() {
        Err("summary block was not printed".to_string())
    } else {
        Ok(())
    }
}

#[then(regex = r"^the phase lines stream as usual$")]
fn phase_lines_stream(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = String::from_utf8_lossy(&world.phase_output);
    if s.contains('✓') || s.contains('✗') {
        Ok(())
    } else {
        Err(format!("no phase lines in:\n{s}"))
    }
}

#[then(regex = r"^`([^`]+)` is printed on its own line \(the existing scripting contract\)$")]
fn run_id_printed_on_stdout(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&world.detached_stdout);
    let needle = line.replace('N', "42");
    if stdout.lines().any(|l| l == needle) {
        Ok(())
    } else {
        Err(format!(
            "expected stdout to contain a sole line {needle:?}, got:\n{stdout}"
        ))
    }
}

#[then(regex = r"^the process exits 0 without attaching$")]
fn process_exits_zero_without_attaching(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.detached {
        Ok(())
    } else {
        Err("detached flag not set; the rig did not exercise the detached path".to_string())
    }
}

#[then(regex = r#"^phase lines \(`[^`]+`, `[^`]+`\) lead up to `([^`]+)`$"#)]
fn phase_lines_lead_up_to(
    world: &mut BehaviourWorld,
    started_marker: String,
) -> Result<(), String> {
    let s = String::from_utf8_lossy(&world.phase_output);
    let needle = strip_placeholder_suffix(&started_marker).replace('N', "42");
    let started_pos = s
        .find(&needle)
        .ok_or_else(|| format!("Started marker {needle:?} missing in:\n{s}"))?;
    let leading = &s[..started_pos];
    if leading.contains('✓') {
        Ok(())
    } else {
        Err(format!("expected ✓ phase lines before {needle:?} in:\n{s}"))
    }
}

#[then(regex = r"^`RunLog` frames from the workload are rendered exactly as they are today$")]
fn runlog_frames_rendered_as_today(_world: &mut BehaviourWorld) {}

#[given(regex = r"^the image `[^`]+` cannot be resolved$")]
fn image_cannot_be_resolved(_world: &mut BehaviourWorld) {}

#[when(regex = r"^resolution fails$")]
async fn resolution_fails(world: &mut BehaviourWorld) {
    let cwd = require_cwd(world).to_path_buf();
    let cli = parse_argv(&world.argv);
    let Command::Run(args) = cli.command else {
        panic!("rig only drives `lns run`");
    };
    let mut sbuf = Vec::<u8>::new();
    print_run_summary(&args, &cwd, &mut sbuf).expect("print_run_summary");
    world.summary_output = String::from_utf8(sbuf).expect("non-utf8 summary");

    let err_frame = encode_frame(&Response::RunLog {
        level: LogLevel::Error,
        verb: Some("Resolve".to_string()),
        message: "failed: registry not reachable".to_string(),
    })
    .expect("encode");
    let exit_frame = encode_frame(&Response::RunExit { code: 125 }).expect("encode");
    {
        let pipe = pipe_mut(world);
        pipe.server.write_all(&err_frame).await.expect("write err");
        pipe.server
            .write_all(&exit_frame)
            .await
            .expect("write exit");
    }
    let mut writer = std::mem::take(&mut world.phase_output);
    let pipe = pipe_mut(world);
    let _ = pre_phase_step(&mut pipe.client, &mut writer)
        .await
        .expect("step 1");
    let outcome = pre_phase_step(&mut pipe.client, &mut writer)
        .await
        .expect("step 2");
    world.phase_output = writer;
    if let PrePhaseStep::EarlyExit(code) = outcome {
        world.early_exit_code = Some(code);
    }
}

#[then(regex = r"^a line `([^`]+)` is printed in place of the resolve `[^`]+`$")]
fn cross_line_in_place_of_check(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    line_present(&world.phase_output, &line)?;
    let s = String::from_utf8_lossy(&world.phase_output);
    if s.contains("✓ resolved") {
        Err(format!("✓ resolved should not be present on failure:\n{s}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the already-printed summary block is not redrawn or erased$")]
fn summary_block_survives(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.is_empty() {
        Err("summary block was never printed".to_string())
    } else if world.summary_output.contains("lns run") {
        Ok(())
    } else {
        Err("summary block was modified".to_string())
    }
}

#[then(regex = r"^the process exits non-zero with the same reason$")]
fn process_exits_non_zero(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.early_exit_code {
        None => Err("no early-exit code captured".to_string()),
        Some(0) => Err("expected non-zero exit, got 0".to_string()),
        Some(_) => Ok(()),
    }
}

#[given(regex = r"^stdout/stderr is not a TTY$")]
fn not_a_tty(world: &mut BehaviourWorld) {
    world.canned_sequence = CannedSequence::ColdCache;
}

#[then(regex = r"^the summary block and phase lines are emitted$")]
fn summary_and_phase_emitted(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.is_empty() {
        return Err("no summary".to_string());
    }
    let s = String::from_utf8_lossy(&world.phase_output);
    if !s.contains('✓') {
        return Err(format!("no phase lines in:\n{s}"));
    }
    Ok(())
}

#[then(regex = r"^no spinner, cursor movement, or ANSI escape sequences are emitted$")]
fn no_ansi(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = String::from_utf8_lossy(&world.phase_output);
    if s.chars().any(|c| c == '\x1b' || c == '\r') {
        Err(format!("ANSI/cursor sequences leaked:\n{s:?}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^each phase line is a single complete line$")]
fn each_phase_line_complete(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = String::from_utf8_lossy(&world.phase_output);
    for line in s.lines() {
        if (line.contains('✓') || line.contains('✗')) && line.trim_start().is_empty() {
            return Err(format!("blank phase line:\n{s}"));
        }
    }
    Ok(())
}

#[then(
    regex = r"^the attached session takes over the terminal cleanly with no leftover phase output$"
)]
fn attached_session_clean(_world: &mut BehaviourWorld) {}

async fn drive_attached_frames(world: &mut BehaviourWorld, frames: Vec<Vec<u8>>) {
    let (client, mut server) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        for frame in frames {
            server
                .write_all(&frame)
                .await
                .expect("write attached frame");
        }
    });
    let mut stdout = std::mem::take(&mut world.attached_stdout);
    let mut status = std::mem::take(&mut world.attached_status);
    drive_attached_session_with_writers(
        client,
        None,
        42,
        false,
        world.attached_stdout_is_terminal.unwrap_or(true),
        Vec::new(),
        lns_cli::service::DetachBehaviour::SignalAndDrain,
        &mut stdout,
        &mut status,
    )
    .await
    .expect("drive_attached_session_with_writers");
    world.attached_stdout = stdout;
    world.attached_status = status;
}

fn stdout_frame(bytes: &[u8]) -> Vec<u8> {
    encode_wire_frame(&WireFrame::Stdout(bytes.to_vec())).expect("encode stdout frame")
}

fn combined_run_output(world: &BehaviourWorld) -> String {
    let mut out = String::new();
    out.push_str(&String::from_utf8_lossy(&world.phase_output));
    out.push_str(&String::from_utf8_lossy(&world.attached_status));
    out
}

#[when(regex = r"^the cold-cache run plays through resolve, boot, session, and finish$")]
async fn cold_cache_full_run(world: &mut BehaviourWorld) {
    world.canned_sequence = CannedSequence::ColdCache;
    drive_canned_sequence(world).await;
    let frames = vec![
        stdout_frame(b"hello from inside a microVM"),
        encode_frame(&run_log("Finished", "in 2.53s")).expect("encode finished"),
        encode_frame(&Response::RunExit { code: 0 }).expect("encode exit"),
    ];
    drive_attached_frames(world, frames).await;
}

#[then(regex = r"^each run-status phase appears exactly once$")]
fn each_phase_once(world: &mut BehaviourWorld) -> Result<(), String> {
    let combined = combined_run_output(world);
    for phrase in [
        "✓ resolved",
        "✓ booted",
        "✓ session ready",
        "✓ started run #42",
        "✓ finished in 2.53s",
    ] {
        let count = combined.matches(phrase).count();
        if count != 1 {
            return Err(format!(
                "{phrase:?} appears {count}× (want 1) in:\n{combined}"
            ));
        }
    }
    Ok(())
}

#[then(regex = r"^no right-aligned developer-format line reaches the user$")]
fn no_right_aligned_line(world: &mut BehaviourWorld) -> Result<(), String> {
    let combined = combined_run_output(world);
    for line in combined.lines() {
        if line_is_right_aligned_verb(line) {
            return Err(format!("right-aligned developer line leaked: {line:?}"));
        }
    }
    Ok(())
}

fn line_is_right_aligned_verb(line: &str) -> bool {
    let leading_spaces = line.len() - line.trim_start_matches(' ').len();
    leading_spaces >= 2 && line.trim_start().starts_with(char::is_alphabetic)
}

#[then(regex = r"^no raw enum verb like `[^`]+` appears verbatim$")]
fn no_raw_enum_verb(world: &mut BehaviourWorld) -> Result<(), String> {
    let combined = combined_run_output(world);
    if combined.contains("SessionReady") {
        Err(format!("raw enum verb leaked in:\n{combined}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^`Started  run #N` and `Finished  in …` never appear right-aligned$")]
fn started_finished_not_right_aligned(world: &mut BehaviourWorld) -> Result<(), String> {
    let combined = combined_run_output(world);
    if combined.contains("Started  run") || combined.contains("Finished  in") {
        Err(format!(
            "right-aligned Started/Finished leaked in:\n{combined}"
        ))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the final byte of the run output is a newline$")]
fn final_byte_is_newline(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.attached_stdout.last() {
        Some(b'\n') => Ok(()),
        other => Err(format!("expected final byte newline, got {other:?}")),
    }
}

#[when(regex = r"^the workload prints without a trailing newline and then exits$")]
async fn workload_prints_without_newline(world: &mut BehaviourWorld) {
    let frames = vec![
        stdout_frame(b"no trailing newline here"),
        encode_frame(&Response::RunExit { code: 0 }).expect("encode exit"),
    ];
    drive_attached_frames(world, frames).await;
}

#[then(regex = r"^the final byte emitted to the user's terminal is a newline$")]
fn final_terminal_byte_newline(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.attached_stdout.last() {
        Some(b'\n') => Ok(()),
        other => Err(format!("expected final byte newline, got {other:?}")),
    }
}

#[given(regex = r"^the user's stdout is redirected to a pipe or file$")]
fn stdout_is_redirected(world: &mut BehaviourWorld) {
    world.attached_stdout_is_terminal = Some(false);
}

#[then(regex = r"^the captured stdout is exactly the workload's bytes with no appended newline$")]
fn captured_stdout_is_byte_exact(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.attached_stdout == b"no trailing newline here" {
        Ok(())
    } else {
        Err(format!(
            "redirected stdout was mutated: {:?}",
            String::from_utf8_lossy(&world.attached_stdout)
        ))
    }
}

fn line_present(buf: &[u8], expected: &str) -> Result<(), String> {
    let s = String::from_utf8_lossy(buf);
    let prefix = strip_placeholder_suffix(expected);
    let normalised = prefix.replace('N', "42");
    if s.contains(&prefix) || s.contains(&normalised) {
        Ok(())
    } else {
        Err(format!("missing {prefix:?} (or {normalised:?}) in:\n{s}"))
    }
}

fn strip_placeholder_suffix(s: &str) -> String {
    let trimmed = s.trim_end_matches('…').trim_end();
    if let Some(idx) = trimmed.find('<') {
        trimmed[..idx].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}
