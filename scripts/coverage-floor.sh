#!/bin/sh
# Per-file 100% coverage gate: every file in the post-strip lcov must be at 100%
# unless it's listed in IGNORES below.
#
# Usage: coverage-floor.sh <lcov.info> [crate-prefix]
#
# Suffix matching: "lns-service/src/relay/adapter.rs" matches any SF: ending
# with that string. A trailing "/" is a directory prefix (matches anywhere in SF).
# <crate-prefix> restricts both SF lines and IGNORES to that crate subtree.

set -eu

lcov=${1:-}
prefix=${2:-}
if [ -z "$lcov" ] || [ ! -f "$lcov" ]; then
    echo "usage: $0 <lcov.info> [crate-prefix]" >&2
    exit 2
fi

# IGNORES: <path-suffix>  <reason> — mandatory reason on every entry.
# Drop an entry when the file reaches 100%. See CLAUDE.md for the full policy.
# Written straight to a temp file (not via $(...)) so reason text isn't constrained by shell quote/backtick parity.
ignores_file=$(mktemp)
trap 'rm -f "$ignores_file"' EXIT
cat > "$ignores_file" <<'EOF'
# Platform-only — host doesn't run the syscalls these files cover.
crates/lns-init/src/main.rs                              binary bootstrap: ExitCode dispatch + RealCmdlineSource leaf; run_host (boot.rs) + mount orchestration (mount.rs) host-tested at 100%
crates/lns-init/src/mount/real.rs                        RealSyscalls libc leaf (mount/chroot/mknod/fexecve/...) + mount_and_exec composition root; orchestration host-tested in mount.rs via FakeSyscalls
crates/lns-artifact/src/resources/host/real.rs           production-wiring leaf reading this host's totals (/sys/devices/system/cpu/present + /proc/meminfo on Linux, available_parallelism + sysctl hw.memsize on macOS); the probe-to-capacity mapping and both parsers are host-tested at 100% in resources/host.rs
crates/lns-service/src/upperfs/real.rs                   provision() composition root: cache::root + rand uuid + clock + spawn_blocking write_ext4; orchestration host-tested via provision_image in upperfs/mod.rs
crates/lns-service/src/connector/registry.rs             the one connector leaf that reaches a registry: caching_registry_for + pull_connector_with for a reference, read_local for a directory. Both arms are one delegation each; pull_connector_with (pin-before-type, kind refusal) is host-tested at 100% in image/mod.rs via FakeRegistry, and read_local (the digest a grant binds to, fileset snapshotting) at 100% in connector/source.rs
crates/lns-service/src/vm/vz/ffi.rs                      Apple Virtualization.framework Obj-C FFI wall (objc2 / dispatch / RcBlock): VM config/boot, the vsock connect_once completion-handler, NSError/NSURL bridging. The connect() backoff/timeout loop is host-tested at 100% in vz.rs via the ConnectOnce port; the per-disk caching/synchronization/read-only policy is host-tested at 100% in vz/attachment.rs.
crates/lns-service/src/vm/diag_console/real.rs           socketpair + tokio::spawn + OwnedFd::from_raw_fd + set_nonblocking fcntl + log-file open + the warn-on-failure wrapper; the read→write→echo→EOF→flush tee loop is host-tested at 100% in diag_console.rs over in-memory readers/writers
crates/lns-service/src/vm/session_client/real.rs         streaming session driver (run_session select/spawn/join concurrency) + run_session_on_fd raw-vsock-fd intake + capture_session_output/capture_session_exec one-shot exec captures (stats probe, tool-provisioner driver) + set_nonblocking; the frame-decode loop (read_server_frames), input→ClientFrame mapping, and the stats parsing (guest_stats.rs) are host-tested at 100%. Exercised end-to-end by the live microVM.
crates/lns-service/src/tools/provisioner/real.rs         MiseProvisioner VM-boot composition root (ensure_pinned engine/companion fetches + provisioner rootfs ingest + descriptor build + vm::boot + broker driver capture over the writable staging bind); driver rendering/parsing, apk expansion, and layer planning are host-tested at 100% in tools/provisioner/mod.rs. Pinned by the @microvm e2e tools feature.
crates/lns-service/src/tools/real.rs                     needs pre_provision_for_pull's logic (registry refusals, libc detection, the ledger-emission loop) moved into tools/mod.rs behind the existing ports, leaving only construction here: cache::root + RealRecordStore/RealToolCache/MiseProvisioner and the warm-vs-serialized-install split. The ensure_tools orchestration it drives is host-tested at 100% in tools/mod.rs; the pull path is pinned by the @microvm e2e tools feature.
crates/lns-service/src/vm/cloud_hypervisor/process.rs    process syscall leaf (spawn/wait/start_kill and virtiofsd --help capture); only runs against the real cloud-hypervisor + virtiofsd binaries. The capability decision and launch sequencing + lifecycle (virtiofsd→socket-wait→relay→CH→transport, kill-on-failure) are host-tested at 100% via injected help and FakeSpawner/FakeChild.
crates/lns-service/src/vm/cloud_hypervisor/real.rs       VMM/vsock syscall leaf: CloudHypervisor::run (spawn_blocking/current-thread-runtime adapter into run_async_with) + the guest-vsock accept loop (UnixListener::accept) and into_blocking_fd (UnixStream into_std/into_raw_fd) whose OS-error arms can't be deterministically driven. run_async_with's orchestration + the connector/handshake logic are host-tested at 100% in cloud_hypervisor.rs and vsock.rs.
crates/lns-service/src/approval_flow/system_font.rs      macOS Core Text leaf (CTFontCreateUIFontForLanguage → kCTFontURLAttribute → CFURL → file read) whose ? error arms can't be deterministically triggered; the pure font-definition building (build_font_defs/apply_host_fonts) is host-tested at 100% in window.rs, and resolution is proven on a real macOS host by system_font's own #[cfg(target_os="macos")] tests.
crates/lns-cli/src/raw_mode/real.rs                      RealTty libc leaf (isatty/tcgetattr/tcsetattr/cfmakeraw/ioctl(TIOCGWINSZ)) + real_tty() wiring + the no-arg production entry points; Tty trait, *_with helpers and RawModeGuard Drop are host-tested at 100% via FakeTty in raw_mode.rs. Pinned end-to-end by the interactive-shell.exp smoke (manual)
crates/lns-session-broker/src/main.rs                    guest broker entry; static-musl PID-1 sibling, only runs inside the microVM. Exit-code clamping lives in exit.rs and is host-tested at 100%.
crates/lns-session-broker/src/pty/real.rs               RealPtySyscalls libc leaf (posix_openpt/grantpt/unlockpt/ptsname/open/setsid/ioctl/getpgrp/tcsetpgrp/tcgetpgrp/dup2) + open_pty/child_setup/foreground_pgrp wiring; allocation+cleanup ordering, error mapping, dup2 loop and pgrp predicate host-tested at 100% in pty.rs via FakePty
crates/lns-session-broker/src/session/real.rs           guest session driver leaf: fork + post-fork child arms + OS-thread pump/drive loops + libc (pty/pipe/signal/exec). The pure decisions (open-session parse, client-frame dispatch, signal targeting, argv validation) and SharedFd / read_client_frame are host-tested at 100% in session.rs.
crates/lns-session-broker/src/vsock/real.rs             RealVsockSyscalls libc leaf (AF_VSOCK socket/bind/listen/accept/read/write) + listen/accept/read_exact/write_all wiring; the listen close-on-failure ordering, accept EINTR loop, sockaddr_vm construction, and read/write partial+EINTR+EOF loops are host-tested at 100% in vsock.rs via FakeVsock
crates/lns-session-broker/src/forker.rs                  LibcForker::fork/wait wrap libc::fork / libc::waitpid (Linux-only syscalls); only the macOS stub branch compiles on this host. The pure exit-status translation lives in exit.rs and is host-tested at 100%.
crates/lns-session-broker/src/network/real.rs           RealCommandRunner/RealFsWriter leaf (std::process::Command + std::fs) + the cfg(linux) bring_up_eth0 composition root; bring_up_eth0_with sequencing/arg-building/error-mapping host-tested at 100% in network.rs via FakeCommandRunner/FakeFsWriter. Pinned end-to-end by the live microVM smoke.
crates/lns-session-broker/src/trust/real.rs             the cfg(linux) seed_trust_store composition root; RealTrustFs's resolving-path probe, read, mkdir and mode-setting write are host-tested against a tempdir here, and the copy-only-when-absent decision is host-tested at 100% in trust.rs via FakeTrustFs. Pinned end-to-end by the live microVM smoke.
crates/lns-session-broker/src/forward/real.rs            serve/accept/splice/pump vsock leaf; `#[cfg(target_os = "linux")]` and only runs inside the microVM. parse_header logic lives in forward/mod.rs at 100%. Pinned end-to-end by the live microVM smoke.
crates/lns-session-broker/src/volumes/real.rs            RealVolumeSyscalls libc leaf (umount / mount MS_REMOUNT|MS_RDONLY) + the cfg(linux) release_volumes composition root; the release order, busy fallback and both-failed reporting are host-tested at 100% in volumes.rs via FakeVolumeSyscalls. Pinned end-to-end by the @microvm volumes scenario.
crates/lns-supervisor/src/main.rs                        guest supervisor entry (tokio bootstrap + network lockdown + privilege drop); only runs inside the microVM
crates/lns-supervisor/src/dispatcher/runtime.rs          agent PTY/fork/termios/SIGWINCH + process-spawn runtime (RealAgentRunner, RealStepRunner, ExitBeforeWorkload); only runs inside the microVM. Host-testable orchestration lives in dispatcher/agent.rs at 100%.
crates/lns-supervisor/src/scripts/ids/real.rs             GuestPasswd leaf: nix NSS lookups of the guest's own passwd/group. The USER[:GROUP] resolution ladder that reads it is host-tested at 100% in scripts/ids.rs against a fake.
crates/lns-supervisor/src/config/mod.rs                  load_config: process env + argv + external lens-sandbox-core loader wiring; the arg-quoting logic is unit-tested in config/quote.rs

# Vendored / host-build glue — not our source.
crates/lns-service/src/composefs/vendor/                 vendored upstream composefs (erofs/fsverity); tracked upstream
crates/lns-service/build.rs                              host cross-build glue (parses kernels.toml)

# Production-wiring adapters — thin pass-throughs; if logic is added, write a unit test and drop the entry.
crates/lns-service/src/browser.rs                        process-spawn leaf (`open`/`xdg-open`); only caller is the tray (itself IGNORES'd GUI).
crates/lns-service/src/download/real.rs                  production wiring over reqwest + tokio::fs (RealFetcher/RealFs/set_mode); pinned by wiremock tests in kernel.rs, the per-traffic UA assertion against a wiremock server in download/mod.rs, and the FakeFs ensure_pinned tests there
crates/lns-service/src/download/traits.rs                trait/type-level declarations; LLVM phantom DA on trait headers
crates/lns-service/src/image/real.rs                     production wiring over oci_client::Client; pinned by fake-Registry tests in image/mod.rs, where the client config (transport + linux/host-arch resolver + the per-verb lns identity) and the login-probe reference it builds from are host-tested at 100%
crates/lns-service/src/artifact/real.rs                  Layer-1 registry/FS composition roots: resolve_for_run/plan_resolved/plan_local/inspect wire the caching registry + content store + real FS, pull_packed_layer builds a RealRegistry for a reference and materialize_filesets wires the real cache root + content store over it, RealSnapshotDir walks a real directory, the pure inspection projection (project_inspection + declared_view_ports) is host-tested at 100% in artifact/inspect.rs, packed-layer reading/size validation and the layer pull itself (pull_packed_layer_with, whose StreamingRegistry tests pin the stream/cleanup/digest-pin ordering) in artifact/fileset.rs, and the planning/assembly/audit logic in artifact/{mod,resources,policy,audit}.rs
crates/lns-cli/src/build/push.rs                         oci_client registry push leaf (auth handshake + push_blob/push_manifest_raw) + the thin registry_auth_for wrapper that loads the real JSON auth store; the client config it pushes with (transport + lns identity) is host-tested at 100% in build/mod.rs, credential selection (select_auth) and the auth error-mapping/recipe logic are host-tested at 100% in build/push_auth.rs, and the artifact it uploads is built by lns_artifact::build (host-tested at 100%)
crates/lns-service/src/guest_tools/real.rs               production wiring over an lns-identified reqwest client for dl-cdn.alpinelinux.org + cache::root() + current_exe() self-hash build id; pinned by fake-Fetcher tests in guest_tools/mod.rs
crates/lns-service/src/supervisor/traits.rs              trait/type-level declarations; LLVM phantom DA on trait headers
crates/lns-service/src/forward/real.rs                   host TCP listener/splice syscall leaf bridging to the guest FORWARD_PORT over the GuestTransport connector (Vz or Cloud Hypervisor); PortForwarder lifecycle + accept-error classification pinned by fake-forwarder and classify_accept_error tests in forward/mod.rs
crates/lns-cli/src/service/real.rs                       leaf adapters (RealPinger, RealChildProbe, send_request, send_cancel_impl, spawn_service_impl) over tokio UnixStream + std::process; loop logic tested via _with + FakePinger/FakeChild
crates/lns-cli/src/artifact/real.rs                      production wiring (the artifact dispatch composition root, push_local over the real registry producer, the reqwest tool-version-index resolver, and RealFs over std::fs); the verb orchestration (run_with_writers and friends) is host-tested at 100% in artifact/mod.rs via the shared CannedService, and the author/distribute logic in artifact/{author,distribute}.rs via MapFs
crates/lns-cli/src/shortcut/real.rs                      production wiring for the two arbitrating shortcuts: opens the service socket and dispatches into whichever namespace `which` settled on; the arbitration itself (owner/resolve/names_a_document) is host-tested at 100% in shortcut.rs
crates/lns-cli/src/sandbox/real.rs                       production wiring (RealSandboxService over UnixStream + send_request, lns_policy file load, dispatch composition root with real tty probes and stdout/stderr writers); the verb orchestration (run_with_writers and friends) is host-tested at 100% in sandbox/mod.rs via the behaviours' FakeSandboxService
crates/lns-cli/src/service/login_agent/real.rs           production-wiring leaf (RealFs over tokio::fs write/remove/create_dir_all/try_exists + RealCommandRunner over tokio::process Command.output()); enable/disable orchestration host-tested at 100% in login_agent.rs via FakeFs/FakeRunner
crates/lns-cli/src/service/login_agent/traits.rs         trait/type-level declarations (Fs, CommandRunner, CommandOutput); LLVM phantom DA on trait headers
crates/lns-cli/src/terminal/real.rs                      RealTerminal leaf: opens /dev/tty for reading and reports whether a controlling terminal exists; a host that is not attached to one can only ever exercise the absent arm. The prompt orchestration it feeds and the answer parsing are host-tested at 100% in terminal.rs and at every prompt site via ScriptedTerminal.
crates/lns-cli/src/platform.rs                         RealUname libc-leaf adapter (libc::uname FFI) + detect() env reads, shared by the update and login wiring; uname_fields_with logic is host-tested in lns-ipc.
crates/lns-service/src/platform.rs                       RealUname libc-leaf adapter (libc::uname FFI) + detect() env reads, feeding the crate's one outbound Identity; uname_fields_with logic is host-tested in lns-ipc.
crates/lns-cli/src/update/real.rs                        pub async fn run wiring (current_exe + RealServiceClient construction + platform::detect) is unmeasured. Pinned end-to-end by manual `lns update` smoke against a real release.
crates/lns-cli/src/uninstall/real.rs                     RealUninstallService/RealLoginAgent/RealFs leaves (send_request over the service socket, disable_login_agent, tokio::fs remove) + build_plan/purge_targets_from_env composition roots (current_exe canonicalize + cache_root/data_root/config/socket/dotfile path resolution + env-override reads) + pub async fn run wiring; the orchestration and the purge-target dir-vs-file decision (purge_targets/push_owned, incl. the override anti-rm-rf guard) are host-tested at 100% in uninstall/mod.rs via fakes. Pinned end-to-end by manual `lns uninstall` smoke.
crates/lns-cli/src/update_check/real.rs                  RealStatusReader leaf (std::fs reads of the service-written update-status.json / install-id under data_root) + run_announce/run_dry_run composition roots; announce/dry_run/classify logic host-tested at 100% in update_check/mod.rs via a FakeReader.
crates/lns-cli/src/login/real.rs                         RealRegistryAuthClient + RealBrowserOpener + RealWebLoginFlow leaves: the service UnixStream RegistryLogin/RegistryLogout/ListRegistryLogins round-trips, the platform browser-opener spawn, and the per-call reqwest device-auth client wiring, plus the run_login/run_logout composition roots (config default-registry read + socket wiring); login/logout/list and web-flow orchestration are host-tested at 100% in login/mod.rs and login/web.rs via fakes. Pinned end-to-end by manual `lns login` smoke.
crates/lns-cli/src/volume/real.rs                        RealVolumeService leaf: one-line delegation to service::real::send_request over the service UnixStream; verb orchestration and rendering are host-tested at 100% in volume/mod.rs via canned/fake services. Exercised end-to-end by Layer 1 e2e-tests' volume lifecycle feature.
crates/lns-cli/src/connector/real.rs                     RealConnectorService leaf: one-line delegation to service::real::send_request over the service UnixStream, plus the `run` composition root (cwd + socket wiring); verb orchestration and rendering are host-tested at 100% in connector/mod.rs via a canned service and the connector_cli behaviours
crates/lns-service/src/update_check/traits.rs            Clock/Fetcher/StateStore port definitions (no executable bodies); implemented by real.rs and exercised via fakes in update_check.rs tests.
crates/lns-service/src/update_check/real.rs              leaf adapters (RealClock SystemTime, RealFetcher reqwest GET with UA header + install_id query param, RealStore std::fs under data_root) + run_periodic interval loop; check_once orchestration host-tested at 100% in update_check.rs via FakeClock/FakeFetcher/FakeStore, and the UA it forwards is host-tested at 100% in identity.rs and lns-ipc.

# Top-level binary mains — exercised by Layer 1 e2e-tests; Layer 1 profraw is excluded from coverage by design.
crates/lns-cli/src/main.rs                               binary bootstrap; covered by Layer 1 e2e-tests, not measured
crates/lns-cli/src/lib.rs                                dispatch composition root (run() match over Command + update-check announce kickoff) lifted from main.rs; exercised by Layer 1 e2e-tests, not measured
crates/lns-service/src/main.rs                           binary bootstrap; covered by Layer 1 e2e-tests, not measured
crates/bump-kernel/src/main.rs                           operator tooling binary: clap dispatch + git / gh / curl / TTY adapter. Manifest semantics + helpers live in bump_kernel::operations which is host-tested at 100%.
crates/bump-mise/src/main.rs                             operator tooling binary: clap dispatch + curl adapter + tarball walk. Signature verification, sha extraction, pin rewrite, and snapshot rendering live in bump_mise::operations which is host-tested at 100%.
crates/coverage-strip-ast/src/main.rs                    tooling binary bootstrap: arg-count guard + std::process::exit + delegate to coverage_strip_ast::run. All strip / classify logic lives in lib.rs and is host-tested at 100%.
crates/lns-service/src/run/orchestrator.rs               top-level run boot sequence (cache::root → guest_tools::ensure → ingest → supervisor → runtime_layer → upperfs → vm::boot → broker session) plus prepare, the composition root for the refusals a run is turned away for before it starts (VsockForwarder + resolve_for_run + workload_identity); the ordering that makes them pre-start is host-tested at 100% in ipc/mod.rs. Exercised end-to-end by Layer 1 e2e-tests; testable shape helpers live in run/mod.rs and are pinned at 100% there.
crates/lns-service/src/relay/adapter.rs                  vsock-fd accept loop + tungstenite WS handshake driver; thin pass-through to OwnedFd::from_raw_fd + tokio_tungstenite::accept_hdr_async_with_config. Auth validation, audit-event detection, inbound-frame dispatch, audit-path resolution, token generation, spawn-time setup, the shared per-run audit budget (AuditBudget), and the cooperative connection supersede (supersede_connection) are all host-tested in relay/mod.rs.
crates/lns-service/src/ipc/adapter.rs                    Unix-socket bind / accept / streaming-request driver (run_server, handle_connection, handle_run, serve_prepared_run, handle_exec, handle_logs, handle_attach, bind_or_replace_stale, bind_secure, set_socket_mode_0600, peer_is_trusted, service_euid, is_instance_alive); the peer_is_authorized uid predicate, the run-start ordering (start_run, which decides every refusal a run can be turned away for before serve_prepared_run registers it) and write_error are host-tested at 100% in ipc/mod.rs. Also register and take_reservations, which only forward four arguments and the await-before-send ordering; the rule they forward (which run takes what was reserved for its name) is host-tested at 100% in connector/real.rs::take_reservations_for. Exercised end-to-end by Layer 1 e2e-tests through the real lns-service socket; handle_request, pump_responses, forward_session_input, stop_run_with, session-input helpers, map_signal, and the exec validate_exec / build_session_params pure helpers are host-tested at 100% in ipc/mod.rs, and the logs/attach follower loop (stream_to) plus the tee (tee_frames) in run_log.rs.
crates/lns-cli/src/service/orchestrator.rs               UnixStream-coupled CLI tail (launch_run, run_image, exec_image, drive_attached_session, drain_to_exit, drain_after_chord, run_stdin_pump, pump_with_detector, run_winsize_forwarder, send_one_shot, dispatch, require_running, real_client). Exercised by Layer 1 e2e-tests through the real lns binary. The pure helpers in this file (plan_feed/drain_pending stdin-pump mapping, render_status_line/render_started_run/phrase_for_verb marker truth-table, needs_final_newline, render_attached_run_log debug-vs-status routing, drive_attached_session_with_writers over in-memory duplex writers, drive_pre_phase/pre_phase_step, published_target, mixins_for_the_run) are host-tested at 100% in this file's own mod tests; preflight_local/preflight_published stay in the coupled tail; the sibling cmd_start/stop/status, require_running_check, socket_path, find_service_binary, parse_signal_name, friendly_started live in service.rs.

# Top-level GUI main-loop adapter — exercised live, not unit-tested.
crates/lns-service/src/tray.rs                           eframe main-loop adapter — `eframe::run_native` takes over the main thread (and on macOS needs a live NSApp), so the `eframe::App` impl and tray/menu wiring can only run live. Card rendering is no longer the blocker: the in-crate headless egui_kittest harness drives real clicks through the AccessKit tree (close ✕, pile pill), and the pure residue (position_top_right arithmetic, visibility_transition, close_action/apply_dismissal) is host-tested; load_icon decode is too. Getting to 100% needs the App impl extracting its per-frame logic behind a seam the harness can drive.
crates/lns-service/examples/approval_preview.rs          dev-only egui preview harness — seeds fixtures and calls the production render_stack so style edits preview live; run manually via `cargo run -p lns-service --example approval_preview`, never run by `cargo test --all-targets`.
crates/lns-service/src/dashboard/mod.rs                  egui render surface — panels/areas/modal/immediate-viewport need a live frame + pointer input (no headless egui harness in-crate). The pure residue (value/label formatting, friendly/relative time bucketing) lives at 100% in dashboard/{filter,sandboxes,live,format}.rs; load/apply_theme/viewport_builder and now_unix_secs are thin wiring exercised by the audit_dashboard example and driven live from tray.rs.
crates/lns-service/examples/audit_dashboard.rs           dev-only egui preview harness — seeds fixtures and calls the production dashboard::render so style edits preview live; run manually via `cargo run -p lns-service --example audit_dashboard`, never run by `cargo test --all-targets`.

# LLVM opener-line phantom DAs — LLVM maps the execution counter for a multi-line construct to
# the first instruction of the body, leaving the opener line with 0 count even when the body runs.
# Drop these entries when coverage-strip-ast is extended to recognise and strip opener lines for
# `} else {`, multi-line struct/enum literals, match-arm `=>` patterns, and `()` closure
# invocation sites.
crates/lns-cli/src/service.rs                            99%+ — `println!` string literal, `} else {` opener, `anyhow::bail!` format-arg lines, named `writeln!` args, and `assert!` message-string lines in full-workspace builds; LLVM maps counters to body lines. Drop when opener-line and macro-arg stripping lands in coverage-strip-ast.
crates/lns-service/src/content_store/mod.rs              99%+ — `})();` closure-invocation expression; LLVM maps counter past the opener. Drop when opener-line stripping lands in coverage-strip-ast.
crates/lns-service/src/ingest.rs                         99%+ — `..Default::default()` struct-update in test helper; LLVM maps counter to initialiser body. Drop when opener-line stripping lands in coverage-strip-ast.
crates/lns-service/src/log.rs                            99%+ — a single multi-line `assert!` format-arg closer (`buf.text(),`) in a test; LLVM maps the counter to the macro body, not the arg line. The local-render tty gate (local_log_layer_accepts target/in_run_scope/stderr_is_tty) and detect_color are host-tested at 100%. Drop when macro-arg-line stripping lands in coverage-strip-ast.
crates/lns-service/src/runtime_layer/mod.rs              99%+ — `} else {` branch-opener and a struct-literal opener inside a test vec; LLVM maps counters to body lines. Drop when opener-line stripping lands in coverage-strip-ast.
EOF

# LF==0 means no executable lines after AST strip — vacuously 100%.
awk -v ignores_path="$ignores_file" -v prefix="$prefix" '
    BEGIN {
        # Build path→reason table; entries not under <prefix> are skipped so each crate owns its own ignores.
        # Reasons are enforced non-empty here so the IGNORES contract can'\''t silently rot.
        lineno = 0
        while ((getline line < ignores_path) > 0) {
            lineno++
            if (line ~ /^[[:space:]]*$/ || line ~ /^[[:space:]]*#/) continue
            # First whitespace-delimited token is the path suffix; rest is the reason.
            i = match(line, /[[:space:]]+/)
            if (i == 0) {
                printf "ERROR coverage-floor.sh: IGNORES line %d has no reason: %s\n", lineno, line > "/dev/stderr"
                exit 2
            }
            path = substr(line, 1, i - 1)
            reason = substr(line, i + RLENGTH)
            sub(/^[[:space:]]+/, "", reason)
            sub(/[[:space:]]+$/, "", reason)
            if (reason == "") {
                printf "ERROR coverage-floor.sh: IGNORES line %d (%s) has empty reason\n", lineno, path > "/dev/stderr"
                exit 2
            }
            if (prefix != "" && index(path, prefix) != 1) continue
            ignore_paths[path] = reason
            ignore_seen[path] = 0
        }
        close(ignores_path)
        fail = 0
        ok_count = 0
        skip_count = 0
        filtered_count = 0
    }

    /^SF:/ { sf = substr($0, 4); lf = 0; lh = 0; uncovered = "" }
    /^LF:/ { lf = substr($0, 4) + 0 }
    /^LH:/ { lh = substr($0, 4) + 0 }
    /^DA:/ {
        rec = substr($0, 4)
        c = index(rec, ",")
        if (c > 0) {
            line_no = substr(rec, 1, c - 1)
            hits = substr(rec, c + 1) + 0
            if (hits == 0) {
                uncovered = (uncovered == "" ? line_no : uncovered "," line_no)
            }
        }
    }

    /^end_of_record/ {
        # index() not suffix-match: we want "SF contains /<prefix>/" semantics.
        if (prefix != "" && index(sf, prefix) == 0) {
            filtered_count++
            next
        }
        ignored = ""
        sf_len = length(sf)
        for (path in ignore_paths) {
            m = length(path)
            if (substr(path, m, 1) == "/") {
                # Directory prefix: match anywhere in SF path.
                if (index(sf, path) > 0) {
                    ignored = path
                    ignore_seen[path] = 1
                    break
                }
            } else if (sf_len >= m && substr(sf, sf_len - m + 1) == path) {
                ignored = path
                ignore_seen[path] = 1
                break
            }
        }
        pct = (lf == 0) ? 100 : (lh / lf * 100)
        if (ignored != "") {
            printf "SKIP  %s: %.2f%% (%d/%d) — %s\n", sf, pct, lh, lf, ignore_paths[ignored]
            skip_count++
        } else if (lf > 0 && lh < lf) {
            printf "FAIL  %s: %.2f%% (%d/%d) — must be 100%% (add to IGNORES with a reason if intentional)\n", \
                sf, pct, lh, lf
            if (uncovered != "") {
                printf "      uncovered lines: %s\n", uncovered
            }
            fail = 1
        } else {
            printf "OK    %s: 100.00%% (%d/%d)\n", sf, lh, lf
            ok_count++
        }
    }

    END {
        # Stale-entry check skipped when prefix is given — the entry may match under another crate'\''s call.
        if (prefix == "") {
            for (path in ignore_paths) {
                if (!ignore_seen[path]) {
                    printf "WARN  IGNORES entry %s did not match any lcov SF: line (file moved or renamed?)\n", path
                }
            }
        }
        label = (prefix == "") ? "workspace" : prefix
        printf "\n[%s] %d at 100%%, %d ignored, %s failures\n", label, ok_count, skip_count, (fail ? "≥1" : "0")
        if (fail) exit 1
    }
' "$lcov"
