pub(crate) mod real;

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

use super::{ProvisionError, ToolRef};
use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

pub const ENGINE_BIN: &str = "/.lens/tools-engine/bin/mise";
pub const CURL_BIN: &str = "/.lens/tools-engine/bin/curl";
pub const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";
pub const DRIVER: &str = "/.lens/tools-engine/provision.sh";
pub const STAGING: &str = "/staging";

pub(crate) struct EngineArtifacts {
    pub mise: PathBuf,
    pub curl: PathBuf,
    pub ca_bundle_pem: Vec<u8>,
    pub companion_specs: Vec<RuntimeFileSpec>,
}

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

/// One shell driver per provision: install each tool with the fail-loud engine, tar its tree into the staging share, and emit one `LNS_TOOL <name> <resolved> <binpath>` marker per success — any failure names its tool with `LNS_FAIL` and stops.
pub(crate) fn render_driver(requests: &[ToolRef]) -> String {
    let mut script = String::from(
        "#!/bin/sh\nset -u\nexport PATH=/.lens/tools-engine/bin:$PATH\nmkdir -p /tmp/mise\n",
    );
    for request in requests {
        let spec = request.to_string();
        let name = &request.name;
        script.push_str(&format!(
            "if ! mise install '{spec}'; then echo \"LNS_FAIL {name}\"; exit 3; fi\n\
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
    pub resolved: String,
    pub bin_path: String,
}

/// Map the driver's stdout back to per-tool outcomes: a complete run yields one result per request; `LNS_FAIL` attributes the failure to its tool with the output tail as the cause; anything else is an engine fault.
pub(crate) fn parse_driver_output(
    stdout: &str,
    exit_code: i32,
    requests: &[ToolRef],
) -> Result<Vec<DriverResult>, ProvisionError> {
    let results: Vec<DriverResult> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.strip_prefix("LNS_TOOL ")?.split_whitespace();
            Some(DriverResult {
                name: parts.next()?.to_string(),
                resolved: parts.next()?.to_string(),
                bin_path: parts.next().unwrap_or(".").to_string(),
            })
        })
        .collect();
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
            cause: output_tail(stdout),
        });
    }
    Err(ProvisionError::Engine(format!(
        "the provisioner driver exited with code {exit_code}: {}",
        output_tail(stdout)
    )))
}

fn output_tail(stdout: &str) -> String {
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.starts_with("LNS_"))
        .collect();
    let tail = lines.len().saturating_sub(8);
    lines[tail..].join(" | ")
}

/// Expand an apk's data files into runtime specs at their canonical guest paths (apk payloads are gzipped tars whose control entries start with a dot).
pub(crate) fn apk_runtime_specs(apk_bytes: &[u8]) -> Result<Vec<RuntimeFileSpec>> {
    let mut specs = Vec::new();
    let mut archive = tar::Archive::new(GzDecoder::new(apk_bytes));
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

/// The CA bundle PEM from the pinned ca-certificates apk, for injection at the canonical path.
pub(crate) fn ca_bundle_pem(apk_bytes: &[u8]) -> Result<Vec<u8>> {
    let specs = apk_runtime_specs(apk_bytes)?;
    for spec in specs {
        if spec.guest_path == CA_BUNDLE
            && let RuntimeSource::Bytes(bytes) = spec.source
        {
            return Ok(bytes);
        }
    }
    bail!("the ca-certificates apk carries no {CA_BUNDLE}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(spec: &str) -> ToolRef {
        lns_artifact::tools::parse(spec).expect("valid tool spec")
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
            parse_driver_output(stdout, 0, &[tool("node@22"), tool("jq@latest")]).unwrap();
        assert_eq!(
            results,
            vec![
                DriverResult {
                    name: "node".into(),
                    resolved: "22.11.0".into(),
                    bin_path: "bin".into(),
                },
                DriverResult {
                    name: "jq".into(),
                    resolved: "1.7.1".into(),
                    bin_path: ".".into(),
                },
            ]
        );
    }

    #[test]
    fn a_failed_install_attributes_the_cause_to_its_tool() {
        let stdout = "mise ERROR fetching https://nodejs.org: timeout\nLNS_FAIL node\n";
        let err = parse_driver_output(stdout, 3, &[tool("node@22")]).unwrap_err();
        match err {
            ProvisionError::FetchFailed { tool, cause } => {
                assert_eq!(tool, "node@22");
                assert!(cause.contains("timeout"), "got: {cause}");
            }
            other => panic!("expected FetchFailed, got {other}"),
        }
    }

    #[test]
    fn a_marker_less_crash_is_an_engine_fault_with_the_output_tail() {
        let err = parse_driver_output("sh: boom\n", 127, &[tool("node@22")]).unwrap_err();
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
            parse_driver_output(stdout, 0, &[tool("node@22"), tool("jq@latest")]).unwrap_err();
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
        let apk = build_apk(&[
            (".PKGINFO", tar::EntryType::Regular, "control"),
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
        ]);
        let specs = apk_runtime_specs(&apk).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].guest_path, "/usr/lib/libstdc++.so.6.0.32");
        assert!(matches!(specs[0].source, RuntimeSource::Bytes(_)));
        assert_eq!(specs[1].guest_path, "/usr/lib/libstdc++.so.6");
        assert!(
            matches!(&specs[1].source, RuntimeSource::Symlink(target) if target == "libstdc++.so.6.0.32")
        );
    }

    #[test]
    fn the_ca_bundle_pem_is_extracted_from_its_canonical_member() {
        let apk = build_apk(&[(
            "etc/ssl/certs/ca-certificates.crt",
            tar::EntryType::Regular,
            "PEM",
        )]);
        assert_eq!(ca_bundle_pem(&apk).unwrap(), b"PEM");
        let empty = build_apk(&[(".PKGINFO", tar::EntryType::Regular, "x")]);
        assert!(ca_bundle_pem(&empty).is_err());
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
