pub(crate) mod real;

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{
    Libc, ProvisionError, ProvisionTarget, SafeVersion, StagedTar, StagedTool, ToolRef, mise,
};
use crate::download::{Fetcher, Fs, PinnedArtifact, ensure_pinned};
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

pub const ENGINE_BIN: &str = "/.lens/tools-engine/bin/mise";
pub const CURL_BIN: &str = "/.lens/tools-engine/bin/curl";
pub const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";
pub const DRIVER: &str = "/.lens/tools-engine/provision.sh";
pub const STAGING: &str = "/staging";
pub const ENGINE_STATE: &str = "/tmp/mise/tools";

#[derive(Debug)]
pub(crate) struct EngineArtifacts {
    pub mise: PathBuf,
    pub curl: PathBuf,
    pub ca_bundle_pem: Vec<u8>,
    pub companion_specs: Vec<RuntimeFileSpec>,
}

const WORKLOAD_COMPANIONS: &[&str] = &["libstdc++", "libgcc"];

/// The provisioner guest's runtime layer: the pinned engine and curl on a reserved path, the CA store at the one path mise reads, the rendered driver, and (musl) the companion library trees at their canonical paths.
pub(crate) fn provisioner_runtime_specs(
    artifacts: &EngineArtifacts,
    driver: String,
) -> Vec<RuntimeFileSpec> {
    let mut specs = vec![
        RuntimeFileSpec {
            guest_path: ENGINE_BIN.to_string(),
            mode: 0o755,
            source: RuntimeSource::HostFile(artifacts.mise.clone()),
        },
        RuntimeFileSpec {
            guest_path: CURL_BIN.to_string(),
            mode: 0o755,
            source: RuntimeSource::HostFile(artifacts.curl.clone()),
        },
        RuntimeFileSpec {
            guest_path: CA_BUNDLE.to_string(),
            mode: 0o644,
            source: RuntimeSource::Bytes(artifacts.ca_bundle_pem.clone()),
        },
        RuntimeFileSpec {
            guest_path: DRIVER.to_string(),
            mode: 0o755,
            source: RuntimeSource::Bytes(driver.into_bytes()),
        },
    ];
    specs.extend(artifacts.companion_specs.iter().cloned());
    specs
}

/// One shell driver per provision: each tool installs under its own engine state so no install resolves a version a sibling left behind, tars its tree into the staging share, and emits one `LNS_TOOL <name> <resolved> <binpath>` marker; any failure names its tool with `LNS_FAIL` and stops.
pub(crate) fn render_driver(requests: &[ToolRef]) -> String {
    let mut script = String::from(
        "#!/bin/sh\nset -u\nexport PATH=/.lens/tools-engine/bin:$PATH\nmkdir -p /tmp/mise/home\n",
    );
    for request in requests {
        let spec = request.to_string();
        let name = &request.name;
        script.push_str(&format!(
            "mkdir -p '{ENGINE_STATE}/{name}/data' '{ENGINE_STATE}/{name}/cache'\n\
             MISE_DATA_DIR='{ENGINE_STATE}/{name}/data'\n\
             MISE_CACHE_DIR='{ENGINE_STATE}/{name}/cache'\n\
             export MISE_DATA_DIR MISE_CACHE_DIR\n\
             if ! mise install '{spec}'; then echo \"LNS_FAIL {name}\"; exit 3; fi\n\
             path=\"$(mise where '{spec}')\" || {{ echo \"LNS_FAIL {name}\"; exit 3; }}\n\
             resolved=\"$(basename \"$path\")\"\n\
             if [ -d \"$path/bin\" ]; then bp=bin; else bp=.; fi\n\
             tar -cf '{STAGING}/{name}.tar' -C \"$path\" . || {{ echo \"LNS_FAIL {name}\"; exit 3; }}\n\
             echo \"LNS_TOOL {name} $resolved $bp\"\n"
        ));
    }
    script.push_str("echo LNS_DONE\n");
    script
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverResult {
    pub name: String,
    pub resolved: SafeVersion,
    pub bin_path: String,
}

/// Map the driver's stdout back to per-tool outcomes: a complete run yields one result per request; `LNS_FAIL` attributes the failure to its tool with the engine's diagnostics as the cause; anything else is an engine fault. Markers are only ever read from stdout — the engine's own chatter is on stderr and must not be able to shadow one.
pub(crate) fn parse_driver_output(
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    requests: &[ToolRef],
) -> Result<Vec<DriverResult>, ProvisionError> {
    let mut results: Vec<DriverResult> = Vec::new();
    for line in stdout.lines() {
        let Some(marker) = line.strip_prefix("LNS_TOOL ") else {
            continue;
        };
        let mut parts = marker.split_whitespace();
        // Exactly three fields: a driver whose `basename` came back empty would otherwise shift the bin path into the version slot and cache the tree under it.
        let (Some(name), Some(resolved), Some(bin_path), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(resolved), true) = (resolved.parse::<SafeVersion>(), is_safe_bin_path(bin_path))
        else {
            return Err(ProvisionError::Engine(format!(
                "the provisioner driver reported an unusable location for {name}: {resolved:?} {bin_path:?}"
            )));
        };
        if results.iter().any(|result| result.name == name) {
            return Err(ProvisionError::Engine(format!(
                "the provisioner driver reported {name} twice"
            )));
        }
        results.push(DriverResult {
            name: name.to_string(),
            resolved,
            bin_path: bin_path.to_string(),
        });
    }
    if exit_code == 0 && stdout.lines().any(|line| line.trim() == "LNS_DONE") {
        for request in requests {
            if !results.iter().any(|result| result.name == request.name) {
                return Err(ProvisionError::Engine(format!(
                    "the provisioner driver reported no result for {request}"
                )));
            }
        }
        return Ok(results);
    }
    if let Some(failed) = stdout
        .lines()
        .find_map(|line| line.strip_prefix("LNS_FAIL ").map(str::trim))
    {
        let tool = requests
            .iter()
            .find(|request| request.name == failed)
            .map(ToolRef::to_string)
            .unwrap_or_else(|| failed.to_string());
        return Err(ProvisionError::FetchFailed {
            tool,
            cause: failure_cause(stdout, stderr),
        });
    }
    Err(ProvisionError::Engine(format!(
        "the provisioner driver exited with code {exit_code}: {}",
        failure_cause(stdout, stderr)
    )))
}

/// The driver's output is guest-supplied and both fields become path components on the host cache and in the guest layer, so they are allowlisted before anything joins them.
fn is_safe_segment(segment: &str) -> bool {
    lns_artifact::tools::is_safe_version(segment)
}

fn is_safe_bin_path(bin_path: &str) -> bool {
    bin_path == "." || bin_path.split('/').all(is_safe_segment)
}

/// The engine writes its diagnostics to stderr, but a bare `sh` failure (a missing driver, a bad redirect) can land on stdout with no marker at all, so that is the fallback.
fn failure_cause(stdout: &str, stderr: &str) -> String {
    let tail = |text: &str| {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty() && !line.starts_with("LNS_"))
            .collect();
        let start = lines.len().saturating_sub(8);
        lines[start..].join(" | ")
    };
    let from_stderr = tail(stderr);
    if from_stderr.is_empty() {
        tail(stdout)
    } else {
        from_stderr
    }
}

/// The libs a provisioned musl binary links, without touching the engine, curl, or the CA store: a warm run needs these two apks and nothing else, and a bump to any other pin must not re-download — let alone refuse — a launch whose tools are already cached.
pub(crate) async fn ensure_workload_companions_with<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    manifest: &mise::Manifest,
    cache_dir: &Path,
    target: &ProvisionTarget,
) -> Result<Vec<RuntimeFileSpec>> {
    let mut specs = Vec::new();
    for companion in manifest
        .companion
        .iter()
        .filter(|companion| WORKLOAD_COMPANIONS.contains(&companion.name.as_str()))
    {
        let apk = ensure_companion_apk(fetcher, fs, companion, cache_dir, target.arch).await?;
        specs.extend(apk_runtime_specs(&apk)?);
    }
    Ok(specs)
}

async fn ensure_companion_apk<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    companion: &mise::Companion,
    cache_dir: &Path,
    arch: super::Arch,
) -> Result<Vec<u8>> {
    let filename = format!("{}-{}.apk", companion.name, companion.version);
    let apk_path = ensure_pinned(
        fetcher,
        fs,
        &companions_root(cache_dir, arch).join("apks"),
        &PinnedArtifact {
            filename: &filename,
            url: &mise::companion_url(companion, arch),
            sha256: mise::companion_sha256(companion, arch)?,
            mode: None,
            label: &companion.name,
        },
    )
    .await?;
    fs.read(&apk_path)
        .await
        .with_context(|| format!("reading {}", apk_path.display()))
}

fn companions_root(cache_dir: &Path, arch: super::Arch) -> PathBuf {
    cache_dir
        .join("tools")
        .join("companions")
        .join(arch.to_string())
}

/// Ensure the pinned engine, static curl, CA bundle, and (musl) companion apks are cached and expanded — every byte sha-verified against the injected manifest before it can reach a guest.
pub(crate) async fn ensure_engine_artifacts_with<F: Fetcher, S: Fs>(
    fetcher: &F,
    fs: &S,
    manifest: &mise::Manifest,
    cache_dir: &Path,
    target: &ProvisionTarget,
) -> Result<EngineArtifacts> {
    let arch = target.arch;
    let root = companions_root(cache_dir, arch);
    let mise_bin = ensure_pinned(
        fetcher,
        fs,
        &root.join("mise").join(&manifest.engine.version),
        &PinnedArtifact {
            filename: "mise",
            url: &manifest.engine_url(arch),
            sha256: manifest.engine_sha256(arch)?,
            mode: Some(0o755),
            label: "mise engine",
        },
    )
    .await?;
    let curl_bin = ensure_pinned(
        fetcher,
        fs,
        &root.join("curl").join(&manifest.static_curl.version),
        &PinnedArtifact {
            filename: "curl",
            url: &manifest.curl_url(arch),
            sha256: manifest.curl_sha256(arch)?,
            mode: Some(0o755),
            label: "static curl",
        },
    )
    .await?;

    let ca_path = ensure_pinned(
        fetcher,
        fs,
        &root.join("ca"),
        &PinnedArtifact {
            filename: &format!("cacert-{}.pem", manifest.ca_bundle.date),
            url: &manifest.ca_bundle.url(),
            sha256: &manifest.ca_bundle.sha256,
            mode: None,
            label: "CA bundle",
        },
    )
    .await?;
    let ca_pem = fs
        .read(&ca_path)
        .await
        .with_context(|| format!("reading {}", ca_path.display()))?;

    let mut companion_specs = Vec::new();
    for companion in &manifest.companion {
        if target.libc != Libc::Musl {
            continue;
        }
        let bytes = ensure_companion_apk(fetcher, fs, companion, cache_dir, arch).await?;
        companion_specs.extend(apk_runtime_specs(&bytes)?);
    }
    Ok(EngineArtifacts {
        mise: mise_bin,
        curl: curl_bin,
        ca_bundle_pem: ca_pem,
        companion_specs,
    })
}

/// Marry the driver's results back to the requests, in declaration order, with provenance from the registry snapshot and the staged tar path each tool was written to.
pub(crate) fn staged_tools_from_results(
    requests: &[ToolRef],
    results: &[DriverResult],
    staging: &Path,
) -> Result<Vec<StagedTool>, ProvisionError> {
    requests
        .iter()
        .map(|request| {
            let result = results
                .iter()
                .find(|result| result.name == request.name)
                .ok_or_else(|| ProvisionError::Engine(format!("no driver result for {request}")))?;
            let backend = crate::tools::registry::backend_for(&request.name)
                .unwrap_or("core:unknown")
                .to_string();
            let source_host = crate::tools::registry::source_host(&request.name, &backend);
            Ok(StagedTool {
                name: request.name.clone(),
                resolved: result.resolved.clone(),
                backend,
                source_host,
                tar: StagedTar::File(staging.join(format!("{}.tar", request.name))),
                bin_paths: vec![result.bin_path.clone()],
            })
        })
        .collect()
}

pub(crate) fn driver_timeout_secs(env_value: Option<&str>) -> u64 {
    const DEFAULT_DRIVER_TIMEOUT_SECS: u64 = 600;
    env_value
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_DRIVER_TIMEOUT_SECS)
}

/// Expand an apk's data files into runtime specs at their canonical guest paths. An apk is several concatenated gzip streams (signature, control, data) whose control entries start with a dot, so the decoder must read across stream boundaries.
pub(crate) fn apk_runtime_specs(apk_bytes: &[u8]) -> Result<Vec<RuntimeFileSpec>> {
    let mut specs = Vec::new();
    let mut archive = tar::Archive::new(flate2::read::MultiGzDecoder::new(apk_bytes));
    archive.set_ignore_zeros(true);
    for entry in archive.entries().context("reading apk")? {
        let mut entry = entry.context("reading apk entry")?;
        let entry_type = entry.header().entry_type();
        let path = entry.path().context("reading apk entry path")?.into_owned();
        let rel = path.to_string_lossy().trim_start_matches("./").to_string();
        if rel.starts_with('.') || rel.is_empty() {
            continue;
        }
        if path.is_absolute() || rel.split('/').any(|segment| segment == "..") {
            bail!("apk entry {rel} escapes the guest root");
        }
        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
        if entry_type.is_dir() {
            continue;
        }
        if entry_type.is_symlink() {
            let target = entry
                .link_name()
                .context("reading apk symlink")?
                .with_context(|| format!("apk symlink {rel} has no target"))?;
            specs.push(RuntimeFileSpec {
                guest_path: format!("/{rel}"),
                mode,
                source: RuntimeSource::Symlink(target.to_string_lossy().into_owned()),
            });
            continue;
        }
        if !entry_type.is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).context("reading apk file")?;
        specs.push(RuntimeFileSpec {
            guest_path: format!("/{rel}"),
            mode,
            source: RuntimeSource::Bytes(bytes),
        });
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(literal: &str) -> lns_artifact::tools::SafeVersion {
        literal.parse().expect("a usable version")
    }

    fn tool(spec: &str) -> ToolRef {
        lns_artifact::tools::parse(spec).expect("valid tool spec")
    }

    #[test]
    fn each_tool_installs_against_its_own_engine_state() {
        // Separate state per tool, so no install resolves a version a sibling's install left behind; a tool that deliberately writes a sibling's dir still can, since they share the guest as root.
        let script = render_driver(&[tool("node@22"), tool("jq@latest")]);
        assert!(
            script.contains("MISE_DATA_DIR='/tmp/mise/tools/node/data'")
                && script.contains("MISE_CACHE_DIR='/tmp/mise/tools/node/cache'"),
            "got: {script}"
        );
        assert!(
            script.contains("MISE_DATA_DIR='/tmp/mise/tools/jq/data'")
                && script.contains("MISE_CACHE_DIR='/tmp/mise/tools/jq/cache'"),
            "got: {script}"
        );
        let node_install = script.find("mise install 'node@22'").unwrap();
        let jq_state = script
            .find("MISE_DATA_DIR='/tmp/mise/tools/jq/data'")
            .unwrap();
        assert!(
            jq_state > node_install,
            "each tool's state is set before its own install, not shared up front"
        );
        assert!(
            !mise::provision_env()
                .iter()
                .any(|(key, _)| key == "MISE_DATA_DIR" || key == "MISE_CACHE_DIR"),
            "no shared data or cache dir survives in the session env"
        );
    }

    #[test]
    fn the_driver_installs_tars_and_marks_each_request_in_order() {
        let script = render_driver(&[tool("node@22"), tool("jq@latest")]);
        assert!(script.starts_with("#!/bin/sh\nset -u\n"), "got: {script}");
        assert!(script.contains("mise install 'node@22'"));
        assert!(script.contains("tar -cf '/staging/node.tar'"));
        assert!(script.contains("mise install 'jq@latest'"));
        assert!(
            script.find("node@22").unwrap() < script.find("jq@latest").unwrap(),
            "declaration order is preserved"
        );
        assert!(script.trim_end().ends_with("echo LNS_DONE"));
    }

    #[test]
    fn a_complete_driver_run_parses_into_per_tool_results() {
        let stdout = "fetching...\nLNS_TOOL node 22.11.0 bin\nLNS_TOOL jq 1.7.1 .\nLNS_DONE\n";
        let results =
            parse_driver_output(stdout, "", 0, &[tool("node@22"), tool("jq@latest")]).unwrap();
        assert_eq!(
            results,
            vec![
                DriverResult {
                    name: "node".into(),
                    resolved: version("22.11.0"),
                    bin_path: "bin".into(),
                },
                DriverResult {
                    name: "jq".into(),
                    resolved: version("1.7.1"),
                    bin_path: ".".into(),
                },
            ]
        );
    }

    #[test]
    fn a_failed_install_attributes_the_cause_to_its_tool() {
        let stdout = "LNS_FAIL node\n";
        let stderr = "mise ERROR fetching https://nodejs.org: timeout\n";
        let err = parse_driver_output(stdout, stderr, 3, &[tool("node@22")]).unwrap_err();
        assert!(
            matches!(&err, ProvisionError::FetchFailed { .. }),
            "got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("node@22") && msg.contains("timeout"),
            "got: {msg}"
        );
    }

    #[test]
    fn a_driver_result_escaping_the_cache_tree_is_refused_before_it_becomes_a_path() {
        for marker in [
            "LNS_TOOL node ../../../../tmp/pwn bin",
            "LNS_TOOL node .. bin",
            "LNS_TOOL node 22.11.0 ../../etc",
            "LNS_TOOL node 22/11 bin",
        ] {
            let err =
                parse_driver_output(&format!("{marker}\nLNS_DONE\n"), "", 0, &[tool("node@22")])
                    .unwrap_err();
            assert!(
                err.to_string().contains("unusable location for node"),
                "marker {marker:?}: got: {err}"
            );
        }
    }

    #[test]
    fn engine_chatter_on_stderr_cannot_shadow_a_marker_or_the_failure_attribution() {
        // Merged into one buffer, a partial stderr line splices onto the next marker and a good install reads as an engine fault.
        let noisy = "downloading node 50%\rmise WARN something\n";
        let results = parse_driver_output(
            "LNS_TOOL node 22.11.0 bin\nLNS_DONE\n",
            noisy,
            0,
            &[tool("node@22")],
        )
        .unwrap();
        assert_eq!(results[0].resolved.as_str(), "22.11.0");

        let err = parse_driver_output("LNS_FAIL node\n", noisy, 3, &[tool("node@22")]).unwrap_err();
        assert!(
            matches!(&err, ProvisionError::FetchFailed { tool, .. } if tool == "node@22"),
            "the per-tool attribution survives noisy output: {err}"
        );
    }

    #[test]
    fn a_bare_shell_failure_with_nothing_on_stderr_still_reports_what_it_printed() {
        let err = parse_driver_output("cannot open provision.sh\n", "", 2, &[tool("node@22")])
            .unwrap_err();
        assert!(
            err.to_string().contains("cannot open provision.sh"),
            "got: {err}"
        );
    }

    #[test]
    fn a_marker_without_exactly_three_fields_registers_nothing() {
        // A two-field line would otherwise read the bin path as the version and cache the tree under it.
        for marker in [
            "LNS_TOOL node",
            "LNS_TOOL node  bin",
            "LNS_TOOL node 22.11.0 bin extra",
        ] {
            let err =
                parse_driver_output(&format!("{marker}\nLNS_DONE\n"), "", 0, &[tool("node@22")])
                    .unwrap_err();
            assert!(
                err.to_string().contains("no result for node@22"),
                "marker {marker:?}: got: {err}"
            );
        }
    }

    #[test]
    fn a_second_marker_for_one_tool_fails_the_whole_provision() {
        // A tool's own install code runs in this guest, so it can forge a sibling's marker — but it cannot suppress the genuine one, and two markers is the tell.
        let stdout =
            "LNS_TOOL jq 6.6.6 bin\nLNS_TOOL node 22.11.0 bin\nLNS_TOOL jq 1.7.1 .\nLNS_DONE\n";
        let err =
            parse_driver_output(stdout, "", 0, &[tool("node@22"), tool("jq@latest")]).unwrap_err();
        assert!(err.to_string().contains("reported jq twice"), "got: {err}");
    }

    #[test]
    fn a_nested_bin_dir_is_still_accepted() {
        let results = parse_driver_output(
            "LNS_TOOL node 22.11.0 libexec/bin\nLNS_DONE\n",
            "",
            0,
            &[tool("node@22")],
        )
        .unwrap();
        assert_eq!(results[0].bin_path, "libexec/bin");
    }

    #[test]
    fn a_marker_less_crash_is_an_engine_fault_with_the_output_tail() {
        let err = parse_driver_output("sh: boom\n", "", 127, &[tool("node@22")]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("infrastructure failed") && msg.contains("127") && msg.contains("boom"),
            "got: {msg}"
        );
    }

    #[test]
    fn a_complete_run_missing_a_request_is_an_engine_fault() {
        let stdout = "LNS_TOOL node 22.11.0 bin\nLNS_DONE\n";
        let err =
            parse_driver_output(stdout, "", 0, &[tool("node@22"), tool("jq@latest")]).unwrap_err();
        assert!(
            err.to_string().contains("no result for jq@latest"),
            "got: {err}"
        );
    }

    fn build_apk(entries: &[(&str, tar::EntryType, &str)]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write as _;
        let mut builder = tar::Builder::new(Vec::new());
        for (path, kind, payload) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_path(path).unwrap();
            header.set_mode(0o644);
            match kind {
                tar::EntryType::Symlink => {
                    header.set_link_name(payload).unwrap();
                    header.set_size(0);
                    header.set_cksum();
                    builder.append(&header, std::io::empty()).unwrap();
                }
                _ => {
                    header.set_size(payload.len() as u64);
                    header.set_cksum();
                    builder.append(&header, payload.as_bytes()).unwrap();
                }
            }
        }
        let tar = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn apk_data_files_land_at_canonical_paths_and_control_entries_are_skipped() {
        // Real apks concatenate gzip streams (signature, control, data); the data files must be reachable behind the dot-named members.
        let mut apk = build_apk(&[(".SIGN.RSA.alpine.rsa.pub", tar::EntryType::Regular, "sig")]);
        apk.extend(build_apk(&[(
            ".PKGINFO",
            tar::EntryType::Regular,
            "control",
        )]));
        apk.extend(build_apk(&[
            ("usr/lib/", tar::EntryType::Directory, ""),
            ("dev/pipe", tar::EntryType::Fifo, ""),
            (
                "usr/lib/libstdc++.so.6.0.32",
                tar::EntryType::Regular,
                "elf",
            ),
            (
                "usr/lib/libstdc++.so.6",
                tar::EntryType::Symlink,
                "libstdc++.so.6.0.32",
            ),
        ]));
        let specs = apk_runtime_specs(&apk).unwrap();
        assert_eq!(
            specs.len(),
            2,
            "dirs, fifos, and control entries are skipped"
        );
        assert_eq!(specs[0].guest_path, "/usr/lib/libstdc++.so.6.0.32");
        assert!(matches!(specs[0].source, RuntimeSource::Bytes(_)));
        assert_eq!(specs[1].guest_path, "/usr/lib/libstdc++.so.6");
        assert!(
            matches!(&specs[1].source, RuntimeSource::Symlink(target) if target == "libstdc++.so.6.0.32")
        );
    }

    #[test]
    fn an_escaping_apk_path_is_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut escaping = tar::Header::new_gnu();
        escaping.set_path("a").unwrap();
        {
            let name = escaping.as_gnu_mut().unwrap();
            let raw = b"usr/../../break";
            name.name[..raw.len()].copy_from_slice(raw);
            name.name[raw.len()] = 0;
        }
        escaping.set_size(1);
        escaping.set_mode(0o644);
        escaping.set_cksum();
        builder.append(&escaping, &b"x"[..]).unwrap();
        let tar = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &tar).unwrap();
        let apk = encoder.finish().unwrap();

        let err = apk_runtime_specs(&apk).unwrap_err();
        assert!(
            format!("{err:#}").contains("escapes the guest root"),
            "got: {err:#}"
        );
    }

    #[test]
    fn staged_tools_marry_results_to_requests_with_registry_provenance() {
        let requests = [tool("node@22"), tool("jq@latest")];
        let results = vec![
            DriverResult {
                name: "jq".into(),
                resolved: version("1.7.1"),
                bin_path: ".".into(),
            },
            DriverResult {
                name: "node".into(),
                resolved: version("22.11.0"),
                bin_path: "bin".into(),
            },
        ];
        let staged = staged_tools_from_results(&requests, &results, Path::new("/staging")).unwrap();
        assert_eq!(staged[0].name, "node", "declaration order wins");
        assert_eq!(staged[0].resolved.as_str(), "22.11.0");
        assert_eq!(staged[0].backend, "core:node");
        assert_eq!(staged[0].source_host.as_deref(), Some("nodejs.org"));
        assert!(
            matches!(&staged[0].tar, StagedTar::File(path) if path == Path::new("/staging/node.tar"))
        );
        assert_eq!(staged[1].name, "jq");
        assert_eq!(staged[1].bin_paths, vec![".".to_string()]);

        let err =
            staged_tools_from_results(&requests, &results[..1], Path::new("/staging")).unwrap_err();
        assert!(err.to_string().contains("node@22"), "got: {err}");
    }

    #[test]
    fn the_driver_timeout_honors_the_override_and_defaults_to_ten_minutes() {
        assert_eq!(driver_timeout_secs(None), 600);
        assert_eq!(driver_timeout_secs(Some("90")), 90);
        assert_eq!(driver_timeout_secs(Some("not a number")), 600);
    }

    mod engine_artifacts {
        use super::*;
        use crate::download::{Fs as FsPort, WritableFile};
        use sha2::{Digest, Sha256};
        use std::collections::{BTreeMap, HashMap};
        use std::sync::{Arc, Mutex};

        struct FakeFetcher {
            responses: HashMap<String, Vec<u8>>,
        }

        impl crate::download::Fetcher for FakeFetcher {
            async fn fetch(&self, url: &str) -> anyhow::Result<Vec<u8>> {
                self.responses
                    .get(url)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unexpected fetch of {url}"))
            }
        }

        #[derive(Default, Clone)]
        struct FakeFs {
            files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
        }

        struct FakeFile {
            path: PathBuf,
            files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
        }

        impl WritableFile for FakeFile {
            async fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
                self.files
                    .lock()
                    .unwrap()
                    .entry(self.path.clone())
                    .or_default()
                    .extend_from_slice(bytes);
                Ok(())
            }
            async fn sync_all(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl FsPort for FakeFs {
            type WritableFile = FakeFile;

            async fn create_dir_all(&self, _p: &Path) -> std::io::Result<()> {
                Ok(())
            }
            async fn is_file(&self, p: &Path) -> bool {
                self.files.lock().unwrap().contains_key(p)
            }
            async fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
                self.files.lock().unwrap().get(p).cloned().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")
                })
            }
            async fn remove_file(&self, p: &Path) -> std::io::Result<()> {
                self.files.lock().unwrap().remove(p);
                Ok(())
            }
            async fn create_new(&self, p: &Path) -> std::io::Result<FakeFile> {
                self.files
                    .lock()
                    .unwrap()
                    .insert(p.to_path_buf(), Vec::new());
                Ok(FakeFile {
                    path: p.to_path_buf(),
                    files: self.files.clone(),
                })
            }
            async fn set_mode(&self, _p: &Path, _mode: u32) -> std::io::Result<()> {
                Ok(())
            }
            async fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                let mut files = self.files.lock().unwrap();
                let bytes = files.remove(from).ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no such file")
                })?;
                files.insert(to.to_path_buf(), bytes);
                Ok(())
            }
        }

        fn sha(bytes: &[u8]) -> String {
            format!("{:x}", Sha256::digest(bytes))
        }

        fn per_arch(reference: &str) -> BTreeMap<String, String> {
            BTreeMap::from([
                ("aarch64".to_string(), reference.to_string()),
                ("x86_64".to_string(), reference.to_string()),
            ])
        }

        fn shas(bytes: &[u8]) -> BTreeMap<String, String> {
            BTreeMap::from([
                ("aarch64".to_string(), sha(bytes)),
                ("x86_64".to_string(), sha(bytes)),
            ])
        }

        fn fixture() -> (mise::Manifest, FakeFetcher) {
            let engine = b"engine-elf".to_vec();
            let curl = b"curl-elf".to_vec();
            let ca_pem = b"PEM".to_vec();
            let lib_apk = build_apk(&[(
                "usr/lib/libstdc++.so.6.0.32",
                tar::EntryType::Regular,
                "elf",
            )]);
            let bash_apk = build_apk(&[("bin/bash", tar::EntryType::Regular, "elf")]);
            let manifest = mise::Manifest {
                engine: mise::Engine {
                    version: "1.0.0".into(),
                    sha256: shas(&engine),
                },
                ca_bundle: mise::CaBundle {
                    date: "2026-07-16".into(),
                    sha256: sha(&ca_pem),
                },
                provisioner_rootfs: mise::ProvisionerRootfs {
                    gnu: per_arch("docker.io/library/debian@sha256:aaaa"),
                    musl: per_arch("docker.io/library/alpine@sha256:bbbb"),
                },
                static_curl: mise::StaticCurl {
                    version: "8.0.0".into(),
                    sha256: shas(&curl),
                },
                companion: vec![
                    mise::Companion {
                        name: "libstdc++".into(),
                        version: "13-r1".into(),
                        sha256: shas(&lib_apk),
                    },
                    mise::Companion {
                        name: "bash".into(),
                        version: "5.2-r0".into(),
                        sha256: shas(&bash_apk),
                    },
                ],
            };
            let arch = crate::tools::Arch::Aarch64;
            let responses = HashMap::from([
                (manifest.engine_url(arch), engine),
                (manifest.curl_url(arch), curl),
                (manifest.ca_bundle.url(), ca_pem),
                (mise::companion_url(&manifest.companion[0], arch), lib_apk),
                (mise::companion_url(&manifest.companion[1], arch), bash_apk),
            ]);
            (manifest, FakeFetcher { responses })
        }

        fn target(libc: Libc) -> ProvisionTarget {
            ProvisionTarget {
                arch: crate::tools::Arch::Aarch64,
                libc,
            }
        }

        #[tokio::test]
        async fn the_fake_fs_mirrors_missing_file_errors() {
            let fs = FakeFs::default();
            assert!(fs.read(Path::new("/missing")).await.is_err());
            assert!(
                fs.rename(Path::new("/missing"), Path::new("/b"))
                    .await
                    .is_err()
            );
        }

        #[tokio::test]
        async fn a_gnu_target_fetches_engine_curl_and_ca_but_no_musl_companions() {
            let (manifest, fetcher) = fixture();
            let fs = FakeFs::default();
            let artifacts = ensure_engine_artifacts_with(
                &fetcher,
                &fs,
                &manifest,
                Path::new("/cache"),
                &target(Libc::Gnu),
            )
            .await
            .unwrap();
            assert_eq!(artifacts.ca_bundle_pem, b"PEM");
            assert!(artifacts.companion_specs.is_empty());
            assert!(fs.is_file(&artifacts.mise).await);
            assert!(fs.is_file(&artifacts.curl).await);
        }

        #[tokio::test]
        async fn a_musl_target_expands_the_companion_trees_at_canonical_paths() {
            let (manifest, fetcher) = fixture();
            let artifacts = ensure_engine_artifacts_with(
                &fetcher,
                &FakeFs::default(),
                &manifest,
                Path::new("/cache"),
                &target(Libc::Musl),
            )
            .await
            .unwrap();
            let paths: Vec<&str> = artifacts
                .companion_specs
                .iter()
                .map(|spec| spec.guest_path.as_str())
                .collect();
            assert_eq!(paths, vec!["/usr/lib/libstdc++.so.6.0.32", "/bin/bash"]);
        }

        #[tokio::test]
        async fn the_workload_companions_need_neither_the_engine_nor_the_ca_store() {
            // A warm musl run took this path; fetching the engine there re-downloaded ~40 MB after a pin bump and refused the launch offline.
            let (manifest, _) = fixture();
            let apks_only = FakeFetcher {
                responses: HashMap::from([(
                    mise::companion_url(&manifest.companion[0], crate::tools::Arch::Aarch64),
                    build_apk(&[(
                        "usr/lib/libstdc++.so.6.0.32",
                        tar::EntryType::Regular,
                        "elf",
                    )]),
                )]),
            };
            let specs = ensure_workload_companions_with(
                &apks_only,
                &FakeFs::default(),
                &manifest,
                Path::new("/cache"),
                &target(Libc::Musl),
            )
            .await
            .expect("an engine that cannot be fetched is not needed here");
            assert_eq!(
                specs
                    .iter()
                    .map(|s| s.guest_path.as_str())
                    .collect::<Vec<_>>(),
                vec!["/usr/lib/libstdc++.so.6.0.32"]
            );
        }

        #[tokio::test]
        async fn only_the_libs_a_tool_links_reach_the_workload_not_the_engines_wrapper_deps() {
            // bash and its libs exist for the engine's own wrapper scripts; injecting them would replace the image's /bin/bash.
            let (manifest, fetcher) = fixture();
            let specs = ensure_workload_companions_with(
                &fetcher,
                &FakeFs::default(),
                &manifest,
                Path::new("/cache"),
                &target(Libc::Musl),
            )
            .await
            .unwrap();
            let paths: Vec<&str> = specs.iter().map(|spec| spec.guest_path.as_str()).collect();
            assert_eq!(paths, vec!["/usr/lib/libstdc++.so.6.0.32"]);
        }

        #[tokio::test]
        async fn a_ca_bundle_whose_bytes_do_not_match_the_pin_is_refused() {
            let (mut manifest, fetcher) = fixture();
            manifest.ca_bundle.sha256 = "a".repeat(64);
            let err = ensure_engine_artifacts_with(
                &fetcher,
                &FakeFs::default(),
                &manifest,
                Path::new("/cache"),
                &target(Libc::Gnu),
            )
            .await
            .unwrap_err();
            assert!(format!("{err:#}").contains("sha256"), "got: {err:#}");
        }
    }

    #[test]
    fn the_provisioner_layer_carries_engine_ca_driver_and_companions() {
        let artifacts = EngineArtifacts {
            mise: PathBuf::from("/cache/mise"),
            curl: PathBuf::from("/cache/curl"),
            ca_bundle_pem: b"PEM".to_vec(),
            companion_specs: vec![RuntimeFileSpec {
                guest_path: "/usr/lib/libstdc++.so.6".into(),
                mode: 0o644,
                source: RuntimeSource::Symlink("libstdc++.so.6.0.32".into()),
            }],
        };
        let specs = provisioner_runtime_specs(&artifacts, "#!/bin/sh\n".into());
        let paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                ENGINE_BIN,
                CURL_BIN,
                CA_BUNDLE,
                DRIVER,
                "/usr/lib/libstdc++.so.6",
            ]
        );
        assert_eq!(specs[0].mode, 0o755);
        assert_eq!(specs[3].mode, 0o755);
    }
}
