use anyhow::{Context, Result, bail};

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::relay::Relay;

mod adapter;
mod real;
mod traits;

const EMBEDDED_NFT: &[u8] = include_bytes!(env!("LNS_NFT_BIN_EMBEDDED"));

fn resolve_nft_bytes(env_get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Vec<u8>> {
    resolve_nft_bytes_inner(env_get, EMBEDDED_NFT)
}

fn resolve_nft_bytes_inner(
    env_get: impl Fn(&str) -> Option<std::ffi::OsString>,
    embedded: &[u8],
) -> Result<Vec<u8>> {
    if let Some(env_val) = env_get("LNS_NFT_BIN").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(env_val);
        if !p.is_file() {
            bail!(
                "LNS_NFT_BIN points at {} which is not a regular file. \
                 Either rebuild the committed binary with \
                 `scripts/build-static-nft.sh linux/arm64` (Docker required), \
                 or unset LNS_NFT_BIN to use the embedded binary.",
                p.display()
            );
        }
        return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
    }
    if embedded.is_empty() {
        bail!(
            "embedded static-nft is empty — build.rs saw LNS_NFT_BIN=<path> \
             at compile time and skipped the embed, but the env var is now \
             unset at runtime. Either set LNS_NFT_BIN again, or rebuild \
             lns-service without it (`unset LNS_NFT_BIN && cargo build \
             -p lns-service --release`)."
        );
    }
    Ok(embedded.to_vec())
}

const EMBEDDED_LNS_SUPERVISOR: &[u8] = include_bytes!(env!("LNS_SUPERVISOR_BIN_EMBEDDED"));

pub(super) fn resolve_embedded_supervisor() -> Option<&'static [u8]> {
    resolve_embedded_supervisor_inner(EMBEDDED_LNS_SUPERVISOR)
}

fn resolve_embedded_supervisor_inner(embedded: &'static [u8]) -> Option<&'static [u8]> {
    (!embedded.is_empty()).then_some(embedded)
}

pub struct SupervisorAssets {
    pub supervisor_bin: PathBuf,
    pub guest_tools_root: PathBuf,
}

pub struct SupervisorSession {
    pub assets: SupervisorAssets,
    pub relay: Relay,
    pub watcher: Option<crate::approval_flow::watcher::PolicyWatcher>,
    // Mirror of `watcher` for the credential state file.
    pub credential_watcher: Option<crate::credential_flow::watcher::CredentialWatcher>,
    // Env vars of the run's custom providers + connected connectors, stripped from `-e` so a real secret can't bypass the placeholder.
    pub managed_env_vars: Vec<String>,
}

impl SupervisorSession {
    #[allow(clippy::too_many_arguments)] // boot entry point threading independent run inputs; a params struct would only relocate the arity
    pub async fn start_if_policy(
        run_id: String,
        microvm_name: String,
        policy: Option<&Path>,
        sandbox_policy: Option<&lns_policy::Policy>,
        sandbox_credentials: &[lns_artifact::spec::CredentialSlot],
        guest_tools_root: PathBuf,
        user_env: Vec<String>,
        workload: lns_policy::grants::WorkloadIdentity,
    ) -> Result<Option<Self>> {
        let Some(policy_path) = policy else {
            if sandbox_policy.is_some() {
                bail!(
                    "this sandbox ships a network policy but no local policy path was resolved to \
                     layer it under; refusing to launch it unsupervised (its policy would go \
                     unenforced)"
                );
            }
            return Ok(None);
        };
        adapter::start(
            run_id,
            microvm_name,
            policy_path,
            sandbox_policy,
            sandbox_credentials,
            guest_tools_root,
            user_env,
            workload,
        )
        .await
        .map(Some)
    }
}

const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

const APPROVAL_TICK: std::time::Duration = std::time::Duration::from_secs(1);

const DEBUG_WRAPPER: &str = r#"#!/bin/sh
exec >/dev/console 2>&1

echo "[wrap] init started, pid=$$, argv=$@"
echo "[wrap] env (sorted):"
env | sort | sed -E 's/^([A-Z0-9_]*(TOKEN|SECRET|PASSWORD|KEY)[A-Z0-9_]*)=.*/\1=<redacted>/' | sed 's/^/[wrap]   /'
echo "[wrap] /run tree:"
ls -la /run /run/lock 2>&1 | sed 's/^/[wrap]   /'
echo "[wrap] /.lens tree:"
ls -la /.lens /.lens/bin 2>&1 | sed 's/^/[wrap]   /'
echo "[wrap] nft probe (supervisor checks /.lens/nft first, PATH second):"
for p in /.lens/nft /.lens/bin/nft /usr/sbin/nft; do
  if [ -e "$p" ]; then
    echo "[wrap]   ls $p -> $(ls -la "$p" 2>&1)"
  fi
done
"$([ -x /.lens/nft ] && echo /.lens/nft || echo nft)" --version 2>&1 | sed 's/^/[wrap]   /'
echo "[wrap]   exit=$?"

echo "[wrap] launching supervisor.real (RUST_LOG=debug to force trace output through is_terminal silence)..."
RUST_LOG=debug /.lens/bin/supervisor.real "$@"
RC=$?
echo "[wrap] supervisor.real exited rc=$RC"
echo "[wrap] sleeping 15s before init exits (so output flushes)"
sleep 15
echo "[wrap] done"
exit "$RC"
"#;

pub fn runtime_specs(
    assets: &SupervisorAssets,
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

    let mut specs = Vec::new();

    if std::env::var_os("LNS_DEBUG_WRAP").is_some() {
        specs.push(RuntimeFileSpec {
            guest_path: "/.lens/bin/supervisor.real".into(),
            mode: 0o755,
            source: RuntimeSource::HostFile(assets.supervisor_bin.clone()),
        });
        specs.push(RuntimeFileSpec {
            guest_path: "/.lens/bin/lns-supervisor".into(),
            mode: 0o755,
            source: RuntimeSource::Bytes(DEBUG_WRAPPER.as_bytes().to_vec()),
        });
    } else {
        specs.push(RuntimeFileSpec {
            guest_path: "/.lens/bin/lns-supervisor".into(),
            mode: 0o755,
            source: RuntimeSource::HostFile(assets.supervisor_bin.clone()),
        });
    }

    specs.push(RuntimeFileSpec {
        guest_path: "/.lens/nft".into(),
        mode: 0o755,
        source: RuntimeSource::Bytes(resolve_nft_bytes(|k| std::env::var_os(k))?),
    });

    walk_guest_tools(&assets.guest_tools_root, &mut specs)?;

    Ok(specs)
}

pub fn guest_tools_only_specs(
    guest_tools_root: &Path,
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    let mut specs = Vec::new();
    walk_guest_tools(guest_tools_root, &mut specs)?;
    specs.extend(imageless_runtime_extras());
    Ok(specs)
}

pub fn guest_tools_tree_specs(
    guest_tools_root: &Path,
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    let mut specs = Vec::new();
    walk_guest_tools(guest_tools_root, &mut specs)?;
    Ok(specs)
}

pub fn imageless_runtime_extras() -> Vec<crate::runtime_layer::RuntimeFileSpec> {
    use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};
    vec![RuntimeFileSpec {
        guest_path: "/bin/sh".into(),
        mode: 0o777,
        source: RuntimeSource::Symlink("/.lens/guest-tools/bin/busybox".into()),
    }]
}

fn walk_guest_tools(
    root: &Path,
    out: &mut Vec<crate::runtime_layer::RuntimeFileSpec>,
) -> Result<()> {
    use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let src = entry.path();
            let rel = src.strip_prefix(root).expect("walked path is under root");
            if rel.as_os_str() == ".ready" {
                continue;
            }
            let guest_path = format!("/{}", rel.display());
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&src)?;
                let target_str = target
                    .to_str()
                    .with_context(|| format!("non-utf8 symlink target at {}", src.display()))?
                    .to_string();
                out.push(RuntimeFileSpec {
                    guest_path,
                    mode: 0o777,
                    source: RuntimeSource::Symlink(target_str),
                });
            } else if meta.is_dir() {
                stack.push(src);
            } else {
                let mode = meta.permissions().mode() & 0o7777;
                out.push(RuntimeFileSpec {
                    guest_path,
                    mode,
                    source: RuntimeSource::HostFile(src),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lns_supervisor_bin_env_var_overrides() {
        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let target_for_stub = target.clone();
        let stub = move |_: &str| Some(target_for_stub.as_os_str().to_os_string());
        let resolved = adapter::ensure_with(stub, None)
            .await
            .expect("ensure with override");
        assert_eq!(resolved, target);
    }

    #[tokio::test]
    async fn lns_supervisor_bin_env_var_rejects_missing_file() {
        let stub = |_: &str| Some(std::ffi::OsString::from("/does/not/exist/lns-supervisor"));
        let err = adapter::ensure_with(stub, None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_SUPERVISOR_BIN"));
        assert!(msg.contains("/does/not/exist/lns-supervisor"));
    }

    #[test]
    fn imageless_runtime_extras_provides_bin_sh_symlink() {
        use crate::runtime_layer::RuntimeSource;
        let extras = imageless_runtime_extras();
        let sh = extras
            .iter()
            .find(|s| s.guest_path == "/bin/sh")
            .expect("/bin/sh in imageless runtime extras");
        let content = &sh.source;
        assert!(
            matches!(content, RuntimeSource::Symlink(_)),
            "/bin/sh source must be a symlink, got {content:?}"
        );
        if let RuntimeSource::Symlink(target) = content {
            assert_eq!(target, "/.lens/guest-tools/bin/busybox");
        }
    }

    #[test]
    fn guest_tools_only_specs_includes_bin_sh_symlink() {
        let d = tempfile::TempDir::new().expect("tempdir");
        let specs = guest_tools_only_specs(d.path()).expect("specs from empty guest-tools tree");
        assert!(specs.iter().any(|s| s.guest_path == "/bin/sh"));
    }

    #[tokio::test]
    async fn start_if_policy_returns_none_when_no_policy() {
        let result = SupervisorSession::start_if_policy(
            "aa42".to_string(),
            "vm-42".into(),
            None,
            None,
            &[],
            PathBuf::from("/tmp"),
            vec![],
            lns_policy::grants::WorkloadIdentity::Adhoc,
        )
        .await
        .unwrap();
        assert!(
            result.is_none(),
            "no policy → unsupervised, no relay spun up"
        );
    }

    #[tokio::test]
    async fn start_if_policy_refuses_a_shipped_policy_with_no_local_path() {
        let mut sandbox_policy = lns_policy::Policy::default();
        sandbox_policy.add_rule(lns_policy::RouteRule::deny_host("api.example.test"));
        let result = SupervisorSession::start_if_policy(
            "aa42".to_string(),
            "vm-42".into(),
            None,
            Some(&sandbox_policy),
            &[],
            PathBuf::from("/tmp"),
            vec![],
            lns_policy::grants::WorkloadIdentity::Adhoc,
        )
        .await;
        let err = result
            .err()
            .expect("a sandbox that ships a policy must not boot unsupervised");
        assert!(
            format!("{err:#}").contains("refusing to launch it unsupervised"),
            "got: {err:#}"
        );
    }

    #[test]
    fn resolve_nft_bytes_honours_env_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fake-nft");
        let payload = b"fake nft ELF body bytes";
        std::fs::write(&path, payload).expect("write");

        let path_for_stub = path.clone();
        let stub = move |_: &str| Some(path_for_stub.as_os_str().to_os_string());
        let bytes = resolve_nft_bytes(stub).expect("override returns the file bytes");
        assert_eq!(bytes.as_slice(), payload);
    }

    #[test]
    fn resolve_nft_bytes_rejects_missing_override_file() {
        let stub = |_: &str| Some(std::ffi::OsString::from("/does/not/exist/nft"));
        let err = resolve_nft_bytes(stub).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_NFT_BIN"));
        assert!(msg.contains("/does/not/exist/nft"));
    }

    #[test]
    fn resolve_nft_bytes_empty_env_then_empty_embedded_bails() {
        let stub = |_: &str| Some(std::ffi::OsString::from(""));
        let err = resolve_nft_bytes_inner(stub, &[]).unwrap_err();
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn resolve_nft_bytes_empty_env_then_non_empty_embedded_returns_embedded() {
        let stub = |_: &str| Some(std::ffi::OsString::from(""));
        let bytes = resolve_nft_bytes_inner(stub, b"embedded-nft-fallback").unwrap();
        assert_eq!(bytes.as_slice(), b"embedded-nft-fallback");
    }

    #[test]
    fn resolve_nft_bytes_returns_embedded_when_no_override_and_non_empty() {
        let stub = |_: &str| None;
        let payload: &[u8] = b"embedded-nft-bytes";
        let bytes = resolve_nft_bytes_inner(stub, payload).expect("embedded bytes");
        assert_eq!(bytes.as_slice(), payload);
    }

    #[test]
    fn resolve_embedded_supervisor_none_when_placeholder() {
        assert!(resolve_embedded_supervisor_inner(&[]).is_none());
    }

    #[test]
    fn resolve_embedded_supervisor_some_when_present() {
        let payload: &[u8] = b"embedded-supervisor-elf";
        assert_eq!(resolve_embedded_supervisor_inner(payload), Some(payload));
    }

    #[test]
    fn walk_guest_tools_skips_dot_ready_marker() {
        let d = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(d.path().join(".ready"), b"").unwrap();
        std::fs::write(d.path().join("kept"), b"keep me").unwrap();
        let specs = guest_tools_only_specs(d.path()).expect("walk");
        let paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        assert!(
            paths.iter().any(|p| p.ends_with("/kept")),
            "non-.ready files must be preserved, got: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("/.ready")),
            ".ready marker must be skipped, got: {paths:?}"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn runtime_specs_uses_debug_wrapper_when_env_set() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::set("LNS_DEBUG_WRAP", "1");
        let d = tempfile::TempDir::new().expect("tempdir");
        let assets = SupervisorAssets {
            supervisor_bin: d.path().join("supervisor.real"),
            guest_tools_root: d.path().to_path_buf(),
        };
        let nft_stub = d.path().join("nft");
        std::fs::write(&nft_stub, b"stub nft").unwrap();
        let _ng = EnvVarGuard::set("LNS_NFT_BIN", &nft_stub);
        std::fs::write(&assets.supervisor_bin, b"").unwrap();
        let specs = runtime_specs(&assets).expect("specs");
        let wrapper = specs
            .iter()
            .find(|s| s.guest_path == "/.lens/bin/lns-supervisor")
            .expect("wrapper spec present");
        let source = &wrapper.source;
        assert!(
            matches!(source, crate::runtime_layer::RuntimeSource::Bytes(_)),
            "LNS_DEBUG_WRAP path must ship inline Bytes wrapper, got {source:?}"
        );
        if let crate::runtime_layer::RuntimeSource::Bytes(body) = source {
            let body_str = std::str::from_utf8(body).expect("wrapper body utf-8");
            let msg = format!("wrapper missing [wrap]: {body_str}");
            assert!(body_str.contains("[wrap]"), "{msg}");
            assert!(
                body_str.contains("(TOKEN|SECRET|PASSWORD|KEY)"),
                "env dump must redact secret-shaped names via a portable ERE: {body_str}"
            );
            assert!(
                body_str.contains("=<redacted>"),
                "env dump must mask secret-shaped values with <redacted>: {body_str}"
            );
            assert!(
                !body_str.contains("env | sort | sed 's/^/[wrap]   /'"),
                "env dump must not be a bare unredacted prefix dump: {body_str}"
            );
        }
        let ships_real = specs
            .iter()
            .any(|s| s.guest_path == "/.lens/bin/supervisor.real");
        assert!(
            ships_real,
            "LNS_DEBUG_WRAP path must also ship the real supervisor at .real"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn runtime_specs_ships_supervisor_elf_directly_when_env_unset() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::unset("LNS_DEBUG_WRAP");
        let d = tempfile::TempDir::new().expect("tempdir");
        let assets = SupervisorAssets {
            supervisor_bin: d.path().join("supervisor"),
            guest_tools_root: d.path().to_path_buf(),
        };
        let nft_stub = d.path().join("nft");
        std::fs::write(&nft_stub, b"stub nft").unwrap();
        let _ng = EnvVarGuard::set("LNS_NFT_BIN", &nft_stub);
        std::fs::write(&assets.supervisor_bin, b"").unwrap();
        let specs = runtime_specs(&assets).expect("specs");
        let canonical = specs
            .iter()
            .find(|s| s.guest_path == "/.lens/bin/lns-supervisor")
            .expect("canonical spec present");
        let source = &canonical.source;
        assert!(
            matches!(source, crate::runtime_layer::RuntimeSource::HostFile(_)),
            "default path must ship supervisor ELF as HostFile, got {source:?}"
        );
        if let crate::runtime_layer::RuntimeSource::HostFile(p) = source {
            assert_eq!(p, &assets.supervisor_bin);
        }
        assert!(
            !specs
                .iter()
                .any(|s| s.guest_path == "/.lens/bin/supervisor.real"),
            "default path must not ship `.real` indirection"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn runtime_specs_ships_static_nft_at_canonical_path() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::unset("LNS_DEBUG_WRAP");
        let d = tempfile::TempDir::new().expect("tempdir");
        let assets = SupervisorAssets {
            supervisor_bin: d.path().join("supervisor"),
            guest_tools_root: d.path().to_path_buf(),
        };
        let nft_stub = d.path().join("nft");
        std::fs::write(&nft_stub, b"stub nft body").unwrap();
        let _ng = EnvVarGuard::set("LNS_NFT_BIN", &nft_stub);
        std::fs::write(&assets.supervisor_bin, b"").unwrap();

        let specs = runtime_specs(&assets).expect("specs");
        let nft = specs
            .iter()
            .find(|s| s.guest_path == "/.lens/nft")
            .expect("/.lens/nft spec present");
        assert_eq!(nft.mode, 0o755);
        let source = &nft.source;
        assert!(
            matches!(source, crate::runtime_layer::RuntimeSource::Bytes(_)),
            "/.lens/nft must be inline Bytes, got {source:?}"
        );
        if let crate::runtime_layer::RuntimeSource::Bytes(b) = source {
            assert_eq!(b.as_slice(), b"stub nft body");
        }
    }

    #[test]
    fn walk_guest_tools_emits_symlink_and_recurses_into_subdirs() {
        let d = tempfile::TempDir::new().expect("tempdir");
        let nested = d.path().join("nested");
        std::fs::create_dir(&nested).expect("nested dir");
        std::fs::write(nested.join("inner"), b"inner-file").expect("inner");
        let link_target = "real-target";
        #[cfg(unix)]
        std::os::unix::fs::symlink(link_target, d.path().join("link")).expect("symlink");
        let mut specs: Vec<crate::runtime_layer::RuntimeFileSpec> = Vec::new();
        walk_guest_tools(d.path(), &mut specs).expect("walk");
        let paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        assert!(
            paths.iter().any(|p| p.ends_with("/nested/inner")),
            "nested file via stack recursion missing; got {paths:?}"
        );
        let link_spec = specs
            .iter()
            .find(|s| s.guest_path == "/link")
            .expect("symlink spec");
        assert!(
            matches!(
                &link_spec.source,
                crate::runtime_layer::RuntimeSource::Symlink(t) if t == link_target
            ),
            "symlink source must preserve target; got {:?}",
            link_spec.source
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn start_if_policy_with_policy_threads_through_adapter_to_supervisor_session() {
        use crate::approval_flow::window::{self, WindowState};
        use crate::test_env::EnvVarGuard;
        window::install(WindowState::new());
        let d = tempfile::TempDir::new().expect("tempdir");
        let _home = EnvVarGuard::set("HOME", d.path());
        let supervisor_bin = d.path().join("supervisor.real");
        std::fs::write(&supervisor_bin, b"fake supervisor").expect("write");
        let _sb = EnvVarGuard::set("LNS_SUPERVISOR_BIN", &supervisor_bin);
        let policy_path = d.path().join("lns-policy.yaml");
        std::fs::write(
            &policy_path,
            "network:\n  allowedRoutes: []\n  defaultVerdict: ask\n",
        )
        .expect("policy");

        let result = SupervisorSession::start_if_policy(
            "deadbeef00000000000000000000aa99".to_string(),
            "calm-finch".into(),
            Some(&policy_path),
            None,
            &[],
            d.path().to_path_buf(),
            vec![],
            lns_policy::grants::WorkloadIdentity::Adhoc,
        )
        .await
        .expect("start_if_policy");
        let session = result.expect("Some(session) with policy set");
        assert_eq!(session.assets.supervisor_bin, supervisor_bin);
        drop(session);
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn start_if_policy_merges_a_sandbox_policy_floor_under_the_local_overlay() {
        use crate::approval_flow::window::{self, WindowState};
        use crate::test_env::EnvVarGuard;
        window::install(WindowState::new());
        let d = tempfile::TempDir::new().expect("tempdir");
        let _home = EnvVarGuard::set("HOME", d.path());
        let supervisor_bin = d.path().join("supervisor.real");
        std::fs::write(&supervisor_bin, b"fake supervisor").expect("write");
        let _sb = EnvVarGuard::set("LNS_SUPERVISOR_BIN", &supervisor_bin);
        let policy_path = d.path().join("lns-policy.yaml");
        std::fs::write(
            &policy_path,
            "network:\n  allowedRoutes: []\n  defaultVerdict: ask\n  defaultTransport: direct\n",
        )
        .expect("policy");
        let mut sandbox_policy = lns_policy::Policy::default();
        sandbox_policy.add_rule(lns_policy::RouteRule::deny_host("api.example.test"));

        let result = SupervisorSession::start_if_policy(
            "deadbeef00000000000000000000aa98".to_string(),
            "calm-finch".into(),
            Some(&policy_path),
            Some(&sandbox_policy),
            &[],
            d.path().to_path_buf(),
            vec![],
            lns_policy::grants::WorkloadIdentity::Adhoc,
        )
        .await
        .expect("start_if_policy");
        let session = result.expect("Some(session) with a sandbox policy floor");
        assert_eq!(session.assets.supervisor_bin, supervisor_bin);
        drop(session);
        tokio::task::yield_now().await;
    }
}
