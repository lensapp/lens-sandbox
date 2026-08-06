use crate::E2eWorld;
use cucumber::{given, then, when};

/// A throwaway GnuPG home with an unprotected key and its own agent, so the scenario signs with a key it created rather than the developer's.
#[given(regex = r"^the host agent holds the key named by user\.signingkey$")]
fn host_agent_with_key(world: &mut E2eWorld) {
    let home = super::microvm::ensure_home(world);
    let gnupg = home.join(".gnupg");
    std::fs::create_dir_all(&gnupg).expect("gnupg home");
    std::fs::set_permissions(&gnupg, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .expect("gnupg home mode");
    std::fs::write(
        gnupg.join("gpg-agent.conf"),
        "allow-loopback-pinentry\ndefault-cache-ttl 3600\n",
    )
    .expect("agent conf");

    let params = gnupg.join("keyparams");
    std::fs::write(
        &params,
        "Key-Type: RSA\nKey-Length: 3072\nKey-Usage: sign\n\
         Name-Real: Lens Sandbox E2E\nName-Email: e2e@example.test\n\
         Expire-Date: 0\n%no-protection\n%commit\n",
    )
    .expect("key params");
    run_gpg(&gnupg, &["--batch", "--gen-key", params.to_str().unwrap()]);

    let key = key_id(&gnupg);
    // The definition-side git identity the projection is expected to carry.
    std::fs::write(
        home.join(".gitconfig"),
        format!(
            "[user]\n\tname = Lens Sandbox E2E\n\temail = e2e@example.test\n\tsigningkey = {key}\n\
             [commit]\n\tgpgsign = true\n"
        ),
    )
    .expect("host gitconfig");

    // Start the agent so its extra socket exists before the run locates it.
    run_gpgconf(&gnupg, &["--launch", "gpg-agent"]);
    world.project_host_access.push("git-signing".into());
}

fn run_gpg(gnupg: &std::path::Path, args: &[&str]) -> String {
    output_of("gpg", gnupg, args)
}

fn run_gpgconf(gnupg: &std::path::Path, args: &[&str]) -> String {
    output_of("gpgconf", gnupg, args)
}

fn output_of(program: &str, gnupg: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(program)
        .args(args)
        .env("GNUPGHOME", gnupg)
        .output()
        .unwrap_or_else(|e| panic!("{program} must be installed to run this scenario: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn key_id(gnupg: &std::path::Path) -> String {
    let listing = run_gpg(gnupg, &["--batch", "--list-secret-keys", "--with-colons"]);
    listing
        .lines()
        .find_map(|line| line.strip_prefix("sec:"))
        .and_then(|rest| rest.split(':').nth(3).map(str::to_string))
        .expect("the generated key must be listed")
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with host access "([^"]+)"$"#)]
fn run_with_host_access(world: &mut E2eWorld, cmd_line: String, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    super::microvm::run_microvm_public(world, vec!["--yes".into()], &cmd_line);
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" as user "([^"]+)" with host access "([^"]+)"$"#
)]
fn run_with_host_access_as_user(world: &mut E2eWorld, cmd_line: String, user: String, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    super::microvm::run_microvm_public(world, vec!["--yes".into(), "-u".into(), user], &cmd_line);
}

#[given(regex = r#"^a sandbox is running with host access "([^"]+)"$"#)]
fn detached_run_with_host_access(world: &mut E2eWorld, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    super::microvm::run_microvm_public(
        world,
        vec!["--yes".into(), "-d".into()],
        "/bin/sh -c 'sleep 120'",
    );
    let run = world
        .last_run_id
        .clone()
        .expect("a detached run id must be captured");
    world.detached_runs.push(run);
}

#[when(regex = r"^the host agent stops$")]
fn host_agent_stops(world: &mut E2eWorld) {
    let gnupg = super::microvm::ensure_home(world).join(".gnupg");
    run_gpgconf(&gnupg, &["--kill", "gpg-agent"]);
}

#[when(regex = r"^the workload attempts a signature$")]
fn workload_attempts_signature(world: &mut E2eWorld) {
    super::microvm::exec_in_last_run(
        world,
        "/bin/sh -c 'echo m > /tmp/m; gpg --batch --yes --detach-sign /tmp/m; echo rc=$?'",
    );
}

#[then(regex = r"^the signature attempt fails$")]
fn signature_attempt_fails(world: &mut E2eWorld) {
    let output = world
        .result
        .as_ref()
        .map(|r| format!("{}{}", r.stdout, r.stderr))
        .unwrap_or_default();
    // Assert the marker arrived first: a bare "no rc=0" passes vacuously when the exec never ran.
    assert!(
        output.contains("rc="),
        "the signing attempt must actually have run: {output}"
    );
    assert!(
        !output.contains("rc=0"),
        "signing must fail once the host agent is gone: {output}"
    );
}

#[then(regex = r"^the sandbox is still running$")]
fn sandbox_still_running(world: &mut E2eWorld) {
    let run = world.last_run_id.clone().expect("a run id");
    let listing = world.run_with_service_env(&["ps"]);
    assert!(
        listing.stdout.contains(&run[..12.min(run.len())]),
        "the sandbox must survive its host agent going away: {}",
        listing.stdout
    );
}

#[then(regex = r"^the output shows the socket owned by the run-as user with mode 0600$")]
fn socket_owner_and_mode(world: &mut E2eWorld) {
    let output = world
        .result
        .as_ref()
        .map(|r| format!("{}{}", r.stdout, r.stderr))
        .unwrap_or_default();
    let line = output
        .lines()
        .find(|l| l.contains("S.gpg-agent"))
        .unwrap_or_else(|| panic!("no listing line for the socket: {output}"));
    assert!(
        line.starts_with("srw-------"),
        "the socket must be a 0600 socket inode: {line}"
    );
    let mut fields = line.split_whitespace();
    let uid = fields.nth(2).unwrap_or_default();
    assert_ne!(
        uid, "0",
        "the socket must belong to the run-as user: {line}"
    );
}
