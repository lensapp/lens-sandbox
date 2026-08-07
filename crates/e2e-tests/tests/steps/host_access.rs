use crate::E2eWorld;
use cucumber::{given, then, when};

/// A throwaway GnuPG home with an unprotected key and its own agent, so the scenario never touches the developer's key.
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

    // Spawned directly rather than via `gpgconf --launch`, which needs gpg-connect-agent and fails on a sandboxed macOS session.
    let agent = std::process::Command::new("gpg-agent")
        .args(["--daemon", "--allow-loopback-pinentry"])
        .env("GNUPGHOME", &gnupg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("gpg-agent must be installed to run this scenario");
    world.host_agent = Some(agent);
    world.host_gnupg_home = Some(gnupg.clone());
    wait_for_agent_socket(&gnupg);

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

    world.project_host_access.push("git-signing".into());
}

fn run_gpg(gnupg: &std::path::Path, args: &[&str]) -> String {
    output_of("gpg", gnupg, args)
}

fn run_gpgconf(gnupg: &std::path::Path, args: &[&str]) -> String {
    output_of("gpgconf", gnupg, args)
}

fn wait_for_agent_socket(gnupg: &std::path::Path) {
    let socket = gnupg.join("S.gpg-agent");
    for _ in 0..100 {
        if socket.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!(
        "gpg-agent did not create {} within 10s; this scenario cannot run without its own agent",
        socket.display()
    );
}

fn output_of(program: &str, gnupg: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(program)
        .args(args)
        .env("GNUPGHOME", gnupg)
        .output()
        .unwrap_or_else(|e| panic!("{program} must be installed to run this scenario: {e}"));
    // A host that refuses to start a gpg-agent (a sandboxed macOS session does) fails here, not later with an opaque "no key".
    assert!(
        out.status.success(),
        "{program} {args:?} failed ({}): {}\nthis scenario needs to start its own gpg-agent in a throwaway GNUPGHOME; a host that refuses that cannot run it",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
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

/// The pinned alpine base ships no gnupg, so a signing scenario installs it first and needs the alpine CDN reachable.
fn allow_gnupg_install(world: &mut E2eWorld) {
    let host = "dl-cdn.alpinelinux.org";
    if !world.project_egress.iter().any(|h| h == host) {
        world.project_egress.push(host.into());
    }
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with host access "([^"]+)"$"#)]
fn run_with_host_access(world: &mut E2eWorld, cmd_line: String, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    allow_gnupg_install(world);
    super::microvm::run_microvm_public(world, vec!["--yes".into()], &cmd_line);
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" as user "([^"]+)" with host access "([^"]+)"$"#
)]
fn run_with_host_access_as_user(world: &mut E2eWorld, cmd_line: String, user: String, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    allow_gnupg_install(world);
    super::microvm::run_microvm_public(world, vec!["--yes".into(), "-u".into(), user], &cmd_line);
}

#[given(regex = r#"^a sandbox is running with host access "([^"]+)"$"#)]
fn detached_run_with_host_access(world: &mut E2eWorld, id: String) {
    if !world.project_host_access.contains(&id) {
        world.project_host_access.push(id);
    }
    allow_gnupg_install(world);
    // The marker lets the later exec tell "gpg is missing" from "signing failed", and lets it wait out an install that a detached run does not block on.
    super::microvm::run_microvm_public(
        world,
        vec!["--yes".into(), "-d".into(), "-u".into(), "root".into()],
        "/bin/sh -c 'apk add --no-cache gnupg >/dev/null && touch /tmp/gpg-ready; sleep 120'",
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
        "/bin/sh -c 'i=0; while [ ! -f /tmp/gpg-ready ] && [ $i -lt 60 ]; do i=$((i+1)); sleep 1; done; \
         if [ ! -f /tmp/gpg-ready ]; then echo GPG_MISSING; else echo m > /tmp/m; gpg --batch --yes --detach-sign /tmp/m; echo rc=$?; fi'",
    );
}

#[then(regex = r"^the signature attempt fails$")]
fn signature_attempt_fails(world: &mut E2eWorld) {
    let output = world
        .result
        .as_ref()
        .map(|r| format!("{}{}", r.stdout, r.stderr))
        .unwrap_or_default();
    // Assert gpg was there and the attempt ran: a bare "no rc=0" passes vacuously on a guest with no gpg at all.
    assert!(
        !output.contains("GPG_MISSING"),
        "gnupg must be installed in the guest, or this scenario proves nothing: {output}"
    );
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
    // Anchored on the mode column: the captured output also carries the CLI's own echo of the command, which names the same path.
    let line = output
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("srw"))
        .unwrap_or_else(|| panic!("no ls listing line for the socket: {output}"));
    assert!(
        line.starts_with("srw-------"),
        "the socket must be mode 0600: {line}"
    );
    let mut fields = line.split_whitespace();
    let uid = fields.nth(2).unwrap_or_default();
    assert_ne!(
        uid, "0",
        "the socket must belong to the run-as user: {line}"
    );
}
