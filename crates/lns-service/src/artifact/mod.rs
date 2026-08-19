use anyhow::{Context, Result, bail};

pub mod assembly;
pub mod audit;
pub mod credential_boot;
pub mod fileset;
pub mod inspect;
pub mod mixin;
pub mod mixin_dir;
pub mod policy;
pub mod real;
pub mod resources;

pub use lns_artifact::{merge, spec};

use assembly::ResolvedSandbox;
use spec::Kind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPath {
    SingleImage,
    Sandbox,
}

pub fn dispatch(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
) -> Result<Option<Kind>> {
    let artifact_type = artifact_type.filter(|t| !t.is_empty());
    let config_media_type = config_media_type.filter(|t| !t.is_empty());
    // Fall back to the config-blob media type only when artifactType is absent (the oras case), never to second-guess a present-but-unrecognized one.
    let kind = match artifact_type {
        Some(t) => Kind::from_artifact_type(t),
        None => config_media_type.and_then(Kind::from_config_media_type),
    };
    match (kind, artifact_type) {
        (Some(kind), _) => Ok(Some(kind)),
        (None, Some(unknown)) => bail!(
            "unsupported artifact type {unknown}; \
             lns run launches a sandbox"
        ),
        (None, None) => Ok(None),
    }
}

/// Classify a run reference and, when the CLI asked to verify it, refuse a plain OCI image that is not a sandbox — pointing at `lns init`. A local sandbox's base image passes `verify=false` and runs directly.
pub fn dispatch_run(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    reference: &str,
    verify: bool,
) -> Result<RunPath> {
    let path = match dispatch(artifact_type, config_media_type)? {
        Some(Kind::Sandbox) => RunPath::Sandbox,
        Some(other) => bail!(
            "a {} artifact is not directly runnable; lns run takes a published sandbox",
            other.as_str()
        ),
        None => RunPath::SingleImage,
    };
    if verify && path == RunPath::SingleImage {
        bail!(
            "{reference} is not a sandbox; run `lns init` to author an lns.yaml, \
             or pass a published sandbox reference"
        );
    }
    Ok(path)
}

/// The baseline the guest's gate folds this directory's live decisions over. A source that ships neither egress nor connectors leaves no baseline at all, so the directory's decisions govern verbatim.
fn baseline_policy(
    egress: &lns_policy::Egress,
    connectors: &[String],
) -> Option<lns_policy::Policy> {
    let ships_policy = *egress != lns_policy::Egress::default() || !connectors.is_empty();
    ships_policy.then(|| lns_policy::Policy {
        network: lns_policy::NetworkPolicy {
            egress: egress.clone(),
        },
        connectors: connectors.to_vec(),
        ..lns_policy::Policy::default()
    })
}

/// Re-base a plan's policy on the egress every source but the directory's own decided, because the gate folds this directory's live decisions over that baseline rather than over the copy of them the resolved document carries (`docs/sandbox-spec.md` §8.1).
pub fn with_authored_baseline(
    mut resolved: ResolvedSandbox,
    authored: &lns_policy::Egress,
) -> ResolvedSandbox {
    let connectors = resolved
        .policy
        .as_ref()
        .map(|policy| policy.connectors.clone())
        .unwrap_or_default();
    resolved.policy = baseline_policy(authored, &connectors);
    resolved
}

/// The baseline a preflight resolved, as it travelled to the run that boots it.
pub fn authored_egress(json: &str) -> Result<lns_policy::Egress> {
    serde_json::from_str(json).context("reading the egress this run's preflight resolved")
}

/// One packed fileset layer as the manifest that carries it declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedLayer {
    pub digest: String,
    pub size: u64,
}

/// The artifact one source document arrived as: the reference that names it, and the fileset layers it carries in manifest order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carrier {
    pub reference: String,
    pub layers: Vec<PackedLayer>,
}

/// Where one guest path's files are pulled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSource {
    pub reference: String,
    pub layer: PackedLayer,
}

/// Every packed fileset a resolution reaches, keyed by the guest path it materializes at.
pub type PackedFilesets = std::collections::BTreeMap<String, PackedSource>;

/// Read back the correlation the resolve answered with: a local run's mixins are pulled by the preflight, so the run carries their coordinates rather than resolving the graph twice.
pub fn packed_from_the_wire(sources: &[lns_ipc::PackedFilesetSource]) -> PackedFilesets {
    sources
        .iter()
        .map(|source| {
            (
                source.guest_path.clone(),
                PackedSource {
                    reference: source.reference.clone(),
                    layer: PackedLayer {
                        digest: source.digest.clone(),
                        size: source.size,
                    },
                },
            )
        })
        .collect()
}

/// §7: a sandbox or mixin artifact carries one layer per `filesets[].path` entry it declares, so a manifest carrying a different number is one no run can correlate — and guessing which entry lost its files is worse than refusing.
pub fn refuse_uncorrelatable_layers(
    reference: &str,
    declared: usize,
    layers: usize,
) -> Result<(), String> {
    if declared == layers {
        return Ok(());
    }
    Err(format!(
        "{reference} declares {declared} path fileset(s) but its artifact carries {layers} layer(s); an artifact carries one layer per entry, so republish it"
    ))
}

/// Correlate each surviving `path` fileset with the layer that carries it: the i-th `path` entry a document declares owns the i-th layer of *that document's* artifact. A merged document interleaves entries from several sources, so position in the result decides nothing — the source's own digest and its own entry order do.
pub fn correlate_packed_filesets(
    origins: &[lns_artifact::merge::FilesetOrigin],
    declared: &std::collections::BTreeMap<String, usize>,
    carriers: &std::collections::BTreeMap<String, Carrier>,
) -> Result<PackedFilesets, Vec<String>> {
    let mut problems: Vec<String> = carriers
        .iter()
        .filter_map(|(label, carrier)| {
            refuse_uncorrelatable_layers(
                &carrier.reference,
                declared.get(label).copied().unwrap_or_default(),
                carrier.layers.len(),
            )
            .err()
        })
        .collect();
    let mut packed = PackedFilesets::new();
    for origin in origins {
        let Some(carrier) = carriers.get(&origin.source) else {
            continue;
        };
        match carrier.layers.get(origin.layer_index) {
            Some(layer) => {
                packed.insert(
                    origin.guest_path.clone(),
                    PackedSource {
                        reference: carrier.reference.clone(),
                        layer: layer.clone(),
                    },
                );
            }
            None => problems.push(format!(
                "{} carries no layer for the fileset mounted at {}",
                carrier.reference, origin.guest_path
            )),
        }
    }
    if problems.is_empty() {
        Ok(packed)
    } else {
        Err(problems)
    }
}

/// Map a flat `kind: sandbox` definition onto a resolved run: its base image plus the inline config, with no component graph to assemble. `packed` decides which `path` entries arrived as a layer of an artifact rather than as a directory on this machine.
pub fn resolved_from_sandbox(
    def: &lns_artifact::sandbox::Definition,
    packed: &PackedFilesets,
) -> ResolvedSandbox {
    ResolvedSandbox {
        base_image: def.spec.image.clone(),
        user: def.spec.user.clone(),
        local_filesets: def
            .spec
            .filesets
            .iter()
            .filter(|fileset| !packed.contains_key(&fileset.guest_path))
            .filter_map(|fileset| {
                fileset.path.as_ref().map(|path| assembly::LocalFileset {
                    source: path.clone(),
                    guest_path: fileset.guest_path.clone(),
                    owner: fileset.owner,
                })
            })
            .collect(),
        host_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset
                    .host_path
                    .as_ref()
                    .map(|source| assembly::HostFileset {
                        source: source.clone(),
                        guest_path: fileset.guest_path.clone(),
                        owner: fileset.owner,
                        optional: fileset.optional,
                    })
            })
            .collect(),
        inline_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset
                    .inline
                    .as_ref()
                    .map(|files| assembly::InlineFileset {
                        files: files.clone(),
                        guest_path: fileset.guest_path.clone(),
                        owner: fileset.owner,
                    })
            })
            .collect(),
        packed_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                packed
                    .get(&fileset.guest_path)
                    .map(|source| assembly::PackedFileset {
                        guest_path: fileset.guest_path.clone(),
                        source: source.clone(),
                        owner: fileset.owner,
                    })
            })
            .collect(),
        command: def.spec.command.clone(),
        env: def.spec.env.clone(),
        resources: def.spec.resources.clone(),
        policy: baseline_policy(&def.spec.egress, &def.spec.connectors),
        credentials: def.spec.credentials.clone(),
        tools: def.spec.tools.clone(),
    }
}

/// The trust gate for a published sandbox's `path` filesets: every one of them arrives packed into a layer of the artifact it was approved at (§3.1.11), so an entry left reading a directory would read the consumer's own filesystem instead of the files the author shipped.
pub fn published_fileset_problems(resolved: &ResolvedSandbox) -> Vec<String> {
    resolved
        .local_filesets
        .iter()
        .map(|local| {
            format!(
                "published sandbox declares the fileset path {} with no layer behind it; publish packs each path directory into a layer of the same artifact",
                local.source
            )
        })
        .collect()
}

/// Plan a published sandbox's config blob: the one place the pulled path turns a document into a run, so every guard between the two applies to a stranger's artifact as much as to a local file.
pub fn plan_published_sandbox(
    config_json: &[u8],
    image_ref: &str,
    packed: &PackedFilesets,
) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .with_context(|| format!("parsing published sandbox {image_ref}"))?;
    if !def.spec.mixins.is_empty() {
        anyhow::bail!(
            "published sandbox {image_ref} reached the plan without being resolved; it still declares mixins ({}), and running it now would drop what they contribute",
            def.spec.mixins.join(", ")
        );
    }
    Ok(resolved_from_sandbox(&def, packed))
}

/// Resolution is what empties this list, so a definition still carrying one never went through it and would boot without what its mixins contribute.
pub fn refuse_unresolved_local_mixins(def: &lns_artifact::sandbox::Definition) -> Result<()> {
    if def.spec.mixins.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "this definition reached the plan without being resolved; it still declares mixins ({})",
        def.spec.mixins.join(", ")
    )
}

/// Plan a local `lns.yaml` definition through the same path a published sandbox takes, so its policy, connectors, and resources apply identically. A published mixin it layers on still brings its filesets packed, so `packed` decides which entries read a directory on this machine.
pub fn plan_local_sandbox(config_json: &[u8], packed: &PackedFilesets) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("parsing the local sandbox definition")?;
    refuse_unresolved_local_mixins(&def)?;
    Ok(resolved_from_sandbox(&def, packed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_local_sandbox_for_tests(config_json: &[u8]) -> Result<ResolvedSandbox> {
        plan_local_sandbox(config_json, &PackedFilesets::new())
    }

    fn plan_published_sandbox_for_tests(
        config_json: &[u8],
        image_ref: &str,
    ) -> Result<ResolvedSandbox> {
        plan_published_sandbox(config_json, image_ref, &PackedFilesets::new())
    }

    fn layer(digest: &str) -> PackedLayer {
        PackedLayer {
            digest: format!("sha256:{}", digest.repeat(64)),
            size: 512,
        }
    }

    fn carrier(reference: &str, layers: &[PackedLayer]) -> Carrier {
        Carrier {
            reference: reference.to_string(),
            layers: layers.to_vec(),
        }
    }

    fn origin(guest_path: &str, source: &str, layer_index: usize) -> merge::FilesetOrigin {
        merge::FilesetOrigin {
            guest_path: guest_path.to_string(),
            source: source.to_string(),
            layer_index,
        }
    }

    #[test]
    fn the_correlation_a_local_runs_preflight_answered_with_survives_the_wire() {
        let packed = packed_from_the_wire(&[lns_ipc::PackedFilesetSource {
            guest_path: "/opt/skills".into(),
            reference: "ghcr.io/acme/skills@sha256:cafe".into(),
            digest: layer("a").digest,
            size: 512,
        }]);
        assert_eq!(
            packed["/opt/skills"],
            PackedSource {
                reference: "ghcr.io/acme/skills@sha256:cafe".into(),
                layer: layer("a"),
            },
            "a local run's mixins are pulled by the preflight, so the boot materializes what that resolve found rather than walking the graph again"
        );
    }

    #[test]
    fn a_packed_fileset_is_pulled_from_the_artifact_of_the_source_that_declared_it() {
        let packed = correlate_packed_filesets(
            &[
                origin("/a", merge::ROOT_LABEL, 0),
                origin("/b", "ghcr.io/acme/obs@sha256:cafe", 0),
            ],
            &[
                (merge::ROOT_LABEL.to_string(), 1),
                ("ghcr.io/acme/obs@sha256:cafe".to_string(), 1),
            ]
            .into(),
            &[
                (
                    merge::ROOT_LABEL.to_string(),
                    carrier("reg/sandbox@sha256:aa", &[layer("a")]),
                ),
                (
                    "ghcr.io/acme/obs@sha256:cafe".to_string(),
                    carrier("ghcr.io/acme/obs@sha256:cafe", &[layer("b")]),
                ),
            ]
            .into(),
        )
        .expect("both sources carry the layer they declared");
        assert_eq!(packed["/a"].reference, "reg/sandbox@sha256:aa");
        assert_eq!(
            packed["/b"].reference, "ghcr.io/acme/obs@sha256:cafe",
            "a mixin's fileset comes out of the mixin's own artifact, which is the digest it was approved at — not out of the sandbox that layered on it"
        );
        assert_eq!(packed["/b"].layer, layer("b"));
    }

    #[test]
    fn a_source_declaring_more_path_filesets_than_its_artifact_carries_refuses_the_run() {
        let problems = correlate_packed_filesets(
            &[origin("/a", merge::ROOT_LABEL, 0)],
            &[(merge::ROOT_LABEL.to_string(), 2)].into(),
            &[(
                merge::ROOT_LABEL.to_string(),
                carrier("reg/sandbox@sha256:aa", &[layer("a")]),
            )]
            .into(),
        )
        .expect_err("one layer per entry is the whole correlation");
        assert_eq!(
            problems,
            [
                "reg/sandbox@sha256:aa declares 2 path fileset(s) but its artifact carries 1 layer(s); an artifact carries one layer per entry, so republish it"
                    .to_string()
            ],
            "guessing which entry lost its files would mount one fileset's content at another's path"
        );
    }

    #[test]
    fn an_entry_whose_layer_index_the_artifact_cannot_answer_refuses_the_run() {
        let problems = correlate_packed_filesets(
            &[origin("/a", merge::ROOT_LABEL, 1)],
            &[(merge::ROOT_LABEL.to_string(), 1)].into(),
            &[(
                merge::ROOT_LABEL.to_string(),
                carrier("reg/sandbox@sha256:aa", &[layer("a")]),
            )]
            .into(),
        )
        .expect_err("a fileset with no layer behind it has no files to mount");
        assert!(
            problems[0].contains("carries no layer for the fileset mounted at /a"),
            "got: {problems:?}"
        );
    }

    #[test]
    fn a_fileset_from_a_source_that_arrived_off_this_machine_carries_no_layer() {
        let packed = correlate_packed_filesets(
            &[origin("/a", "/work/mixins/pg", 0)],
            &[("/work/mixins/pg".to_string(), 1)].into(),
            &std::collections::BTreeMap::new(),
        )
        .expect("a directory mixin has no artifact to pull from");
        assert!(
            packed.is_empty(),
            "a directory this machine read has its files on this machine, so nothing is fetched for it"
        );
    }

    #[test]
    fn resolved_from_sandbox_splits_packed_and_on_disk_path_filesets() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"/work/skills","guestPath":"/a"},{"path":"./shipped","guestPath":"/b","owner":"root"}]}}"#,
        )
        .unwrap();
        let packed: PackedFilesets = [(
            "/b".to_string(),
            PackedSource {
                reference: "reg/sandbox@sha256:aa".into(),
                layer: layer("b"),
            },
        )]
        .into();
        let resolved = resolved_from_sandbox(&def, &packed);
        assert_eq!(
            resolved.local_filesets,
            [assembly::LocalFileset {
                source: "/work/skills".into(),
                guest_path: "/a".into(),
                owner: lns_artifact::sandbox::FilesetOwner::Workload,
            }],
            "an entry with no layer behind it is a local directory read at launch"
        );
        assert_eq!(
            resolved.packed_filesets,
            [assembly::PackedFileset {
                guest_path: "/b".into(),
                source: packed["/b"].clone(),
                owner: lns_artifact::sandbox::FilesetOwner::Root,
            }],
            "a declared owner must survive resolution"
        );
    }

    #[test]
    fn published_fileset_problems_refuse_a_path_no_layer_carries() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/a"}]}}"#,
        )
        .unwrap();
        let problems =
            published_fileset_problems(&resolved_from_sandbox(&def, &PackedFilesets::new()));
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        assert!(
            problems[0].contains("the fileset path ./skills with no layer behind it"),
            "a pulled document's path names a directory in the artifact, not on the consumer's disk — reading theirs would mount whatever happens to sit there; got: {problems:?}"
        );
    }

    #[test]
    fn published_fileset_problems_pass_a_path_its_artifact_carries() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/a"}]}}"#,
        )
        .unwrap();
        let packed: PackedFilesets = [(
            "/a".to_string(),
            PackedSource {
                reference: "reg/sandbox@sha256:aa".into(),
                layer: layer("a"),
            },
        )]
        .into();
        assert!(
            published_fileset_problems(&resolved_from_sandbox(&def, &packed)).is_empty(),
            "a path entry is the shape a published document ships (docs/sandbox-spec.md §6), so the gate must admit it"
        );
    }

    #[test]
    fn an_empty_artifact_type_and_config_type_is_treated_as_a_plain_image() {
        assert_eq!(dispatch(Some(""), Some("")).unwrap(), None);
    }

    #[test]
    fn dispatch_run_refuses_a_plain_image_reference_pointing_at_lns_init() {
        let err = dispatch_run(None, None, "alpine:3.20", true).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not a sandbox"), "got: {msg}");
        assert!(msg.contains("lns init"), "got: {msg}");
    }

    #[test]
    fn resolved_from_sandbox_carries_the_base_image_command_env_and_policy() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base@sha256:abc","command":"agent --serve","env":{"MODE":"research"},"egress":{"http":[{"match":"*","verdict":"deny"}]},"connectors":["some-provider"],"user":"root"}}"#,
        )
        .unwrap();
        let resolved = resolved_from_sandbox(&def, &PackedFilesets::new());
        assert_eq!(resolved.base_image, "ghcr.io/team/base@sha256:abc");
        assert_eq!(resolved.command.as_deref(), Some("agent --serve"));
        assert_eq!(
            assembly::assemble(&resolved).user.as_deref(),
            Some("root"),
            "the run-as user the definition asked for has to survive assembly, or the launch resolves without it"
        );
        assert_eq!(
            resolved.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert!(resolved.packed_filesets.is_empty());
        let policy = resolved
            .policy
            .expect("a flat sandbox carries its inline policy");
        assert_eq!(
            policy.network.egress.http[0].match_pattern, "*",
            "the baseline's lockdown is a catch-all deny it carries in the table"
        );
        assert_eq!(policy.connectors, vec!["some-provider".to_string()]);
    }

    #[test]
    fn a_plan_re_based_on_the_authored_egress_leaves_out_what_the_directory_decided() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","connectors":["some-provider"],"egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"},{"match":"api.some-vendor.example","verdict":"deny"}]}}}"#,
        )
        .unwrap();
        let authored = authored_egress(
            r#"{"http":[{"match":"api.some-vendor.example","verdict":"deny"}],"tcp":[]}"#,
        )
        .expect("the baseline a preflight resolved parses");

        let policy = with_authored_baseline(
            resolved_from_sandbox(&def, &PackedFilesets::new()),
            &authored,
        )
        .policy
        .expect("a sandbox that ships egress keeps a baseline");

        assert_eq!(
            policy
                .network
                .egress
                .http
                .iter()
                .map(|rule| rule.match_pattern.as_str())
                .collect::<Vec<_>>(),
            ["api.some-vendor.example"],
            "the gate folds the live decisions file over this, and the developer's own allow frozen into it would outlive them deleting the rule"
        );
        assert_eq!(
            policy.connectors,
            vec!["some-provider".to_string()],
            "no mixin can name a connector, so re-basing the egress must not drop the list the sandbox itself declared"
        );
    }

    #[test]
    fn a_plan_whose_only_egress_was_the_directorys_own_leaves_the_gate_no_baseline() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"}]}}}"#,
        )
        .unwrap();

        assert_eq!(
            with_authored_baseline(
                resolved_from_sandbox(&def, &PackedFilesets::new()),
                &lns_policy::Egress::default(),
            )
            .policy,
            None,
            "nothing but the developer decided anything here, so the file governs verbatim rather than through a merge with itself"
        );
    }

    #[test]
    fn a_definition_that_shipped_nothing_still_leaves_the_gate_no_baseline_to_fold_over() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#,
        )
        .unwrap();

        assert_eq!(
            with_authored_baseline(
                resolved_from_sandbox(&def, &PackedFilesets::new()),
                &lns_policy::Egress::default(),
            )
            .policy,
            None,
            "a directory whose decisions are the only word on the network has nothing to layer them over, and inventing an empty baseline would merge the file with itself"
        );
    }

    #[test]
    fn a_baseline_that_is_not_an_egress_table_refuses_rather_than_governing_as_nothing() {
        let err = authored_egress("not json").unwrap_err();
        assert!(
            format!("{err:#}").contains("the egress this run's preflight resolved"),
            "a baseline read as empty would silently drop every rule the sandbox ships; got: {err:#}"
        );
    }

    #[test]
    fn a_definition_shipping_no_policy_or_connectors_plans_without_a_baseline() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#,
        )
        .unwrap();
        assert_eq!(
            resolved_from_sandbox(&def, &PackedFilesets::new()).policy,
            None,
            "a plain definition must leave the directory overlay governing verbatim"
        );
    }

    #[test]
    fn plan_local_sandbox_resolves_the_definition_like_a_published_one() {
        let resolved = plan_local_sandbox_for_tests(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","connectors":["some-provider"],"resources":{"cpu":2,"memory":"1Gi"}}}"#,
        )
        .unwrap();
        assert_eq!(resolved.base_image, "ghcr.io/team/base:1");
        assert!(
            resolved.resources.is_some(),
            "resources must survive the plan"
        );
        let policy = resolved.policy.expect("the plan carries the inline policy");
        assert_eq!(policy.connectors, vec!["some-provider".to_string()]);
    }

    #[test]
    fn plan_local_sandbox_surfaces_a_broken_definition() {
        let err = plan_local_sandbox_for_tests(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{}}"#,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_published_document_reaching_the_plan_unresolved_refuses_rather_than_dropping_its_mixins() {
        let pinned = format!("ghcr.io/acme/postgres-tools@sha256:{}", "c".repeat(64));
        let err = plan_published_sandbox_for_tests(
            format!(
                r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"ghcr.io/team/base:1","mixins":["{pinned}"]}}}}"#
            )
            .as_bytes(),
            "registry.example.test/team/sandbox:1",
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("reached the plan without being resolved"),
            "resolution is what empties this list, so a document that still carries one never went through it and would boot without what its mixins contribute; got: {err:#}"
        );
    }

    #[test]
    fn dispatch_runs_a_published_sandbox_artifact_directly() {
        let sandbox_type = Kind::Sandbox.artifact_type();
        assert_eq!(
            dispatch(Some(&sandbox_type), None).unwrap(),
            Some(Kind::Sandbox)
        );
        assert_eq!(
            dispatch_run(Some(&sandbox_type), None, "ghcr.io/team/hermes:1", true).unwrap(),
            RunPath::Sandbox,
            "a published sandbox is the one runnable noun, verified or not"
        );
    }

    #[test]
    fn dispatch_run_runs_a_local_base_image_directly_when_verification_is_off() {
        assert_eq!(
            dispatch_run(None, None, "docker.io/library/alpine:3.20", false).unwrap(),
            RunPath::SingleImage,
            "a local sandbox's plain base image must run directly, not be refused",
        );
    }

    #[test]
    fn a_present_unknown_artifact_type_is_refused_even_with_a_known_config_media_type() {
        let err = dispatch(
            Some("application/vnd.oci.image.config.v1+json"),
            Some(&Kind::Sandbox.config_media_type()),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unsupported artifact type"),
            "a real artifactType we don't know must not be overridden by the config blob: {err:#}"
        );
    }
}
