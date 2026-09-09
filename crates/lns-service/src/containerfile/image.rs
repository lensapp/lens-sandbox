use anyhow::{Context, Result, bail};
use oci_client::manifest::{OciDescriptor, OciImageManifest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::executor::ConfigDraft;
use super::tar_layer::{LAYER_MEDIA_TYPE, LayerBlob};

/// What the base image contributes: the manifest and the config document exactly as the registry served them.
pub(crate) struct ParentImage {
    pub manifest: OciImageManifest,
    pub config: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BuiltImage {
    pub manifest: OciImageManifest,
    pub manifest_digest: String,
    pub config: String,
}

pub(crate) fn assemble(
    parent: &ParentImage,
    layer: Option<&LayerBlob>,
    draft: &ConfigDraft,
    created_by: &str,
    created: &str,
) -> Result<BuiltImage> {
    let config = config_over(&parent.config, layer, draft, created_by, created)?;
    let config_digest = format!("sha256:{}", hex::encode(Sha256::digest(config.as_bytes())));

    let mut manifest = parent.manifest.clone();
    manifest.config = OciDescriptor {
        media_type: parent.manifest.config.media_type.clone(),
        digest: config_digest,
        size: config.len() as i64,
        ..Default::default()
    };
    if let Some(layer) = layer {
        manifest.layers.push(OciDescriptor {
            media_type: LAYER_MEDIA_TYPE.to_string(),
            digest: layer.digest.clone(),
            size: layer.size() as i64,
            ..Default::default()
        });
    }
    manifest.artifact_type = None;

    let manifest_bytes = serde_json::to_vec(&manifest).context("serializing the built manifest")?;
    Ok(BuiltImage {
        manifest,
        manifest_digest: format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes))),
        config,
    })
}

fn config_over(
    parent: &str,
    layer: Option<&LayerBlob>,
    draft: &ConfigDraft,
    created_by: &str,
    created: &str,
) -> Result<String> {
    let mut config: Value =
        serde_json::from_str(parent).context("parsing the base image's config")?;
    let object = config
        .as_object_mut()
        .context("the base image's config is not a JSON object")?;
    object.insert("created".to_string(), json!(created));

    let diff_ids = object
        .get_mut("rootfs")
        .and_then(Value::as_object_mut)
        .and_then(|rootfs| rootfs.get_mut("diff_ids"))
        .and_then(Value::as_array_mut)
        .context("the base image's config declares no rootfs.diff_ids to build on")?;
    if let Some(layer) = layer {
        diff_ids.push(json!(layer.digest));
    }

    let history = object
        .entry("history".to_string())
        .or_insert_with(|| json!([]));
    let Some(history) = history.as_array_mut() else {
        bail!("the base image's config has a history that is not a list");
    };
    let mut entry = json!({ "created": created, "created_by": created_by });
    if layer.is_none() {
        entry["empty_layer"] = json!(true);
    }
    history.push(entry);

    let runtime = object
        .entry("config".to_string())
        .or_insert_with(|| json!({}));
    let Some(runtime) = runtime.as_object_mut() else {
        bail!("the base image's config has a config block that is not an object");
    };
    declared(runtime, draft);

    serde_json::to_string(&config).context("serializing the built config")
}

/// What the Containerfile said about the image, over what its base said.
fn declared(runtime: &mut serde_json::Map<String, Value>, draft: &ConfigDraft) {
    if !draft.env.is_empty() {
        runtime.insert("Env".to_string(), json!(merged_env(runtime, &draft.env)));
    }
    if !draft.labels.is_empty() {
        let mut labels = match runtime.get("Labels").and_then(Value::as_object) {
            Some(existing) => existing.clone(),
            None => serde_json::Map::new(),
        };
        for (key, value) in &draft.labels {
            labels.insert(key.clone(), json!(value));
        }
        runtime.insert("Labels".to_string(), Value::Object(labels));
    }
    if let Some(user) = &draft.user {
        runtime.insert("User".to_string(), json!(user));
    }
    if let Some(workdir) = &draft.workdir {
        runtime.insert("WorkingDir".to_string(), json!(workdir));
    }
    if let Some(entrypoint) = &draft.entrypoint {
        runtime.insert("Entrypoint".to_string(), json!(entrypoint));
        // Docker drops the base image's CMD when a Containerfile sets ENTRYPOINT without its own.
        if draft.cmd.is_none() {
            runtime.insert("Cmd".to_string(), Value::Null);
        }
    }
    if let Some(cmd) = &draft.cmd {
        runtime.insert("Cmd".to_string(), json!(cmd));
    }
    if let Some(shell) = &draft.shell {
        runtime.insert("Shell".to_string(), json!(shell));
    }
    if !draft.exposed_ports.is_empty() {
        runtime.insert(
            "ExposedPorts".to_string(),
            keyed(
                runtime.get("ExposedPorts"),
                draft.exposed_ports.iter().map(with_protocol),
            ),
        );
    }
    if !draft.volumes.is_empty() {
        runtime.insert(
            "Volumes".to_string(),
            keyed(runtime.get("Volumes"), draft.volumes.iter().cloned()),
        );
    }
}

/// `Env` is a list of assignments, so one key the Containerfile sets replaces the base's value in place.
fn merged_env(
    runtime: &serde_json::Map<String, Value>,
    declared: &[(String, String)],
) -> Vec<String> {
    let mut env: Vec<String> = runtime
        .get("Env")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for (key, value) in declared {
        let assignment = format!("{key}={value}");
        match env
            .iter_mut()
            .find(|entry| entry.split('=').next() == Some(key.as_str()))
        {
            Some(existing) => *existing = assignment,
            None => env.push(assignment),
        }
    }
    env
}

/// `ExposedPorts` and `Volumes` are sets the OCI config spells as objects with empty values.
fn keyed(existing: Option<&Value>, keys: impl Iterator<Item = String>) -> Value {
    let mut set = match existing.and_then(Value::as_object) {
        Some(existing) => existing.clone(),
        None => serde_json::Map::new(),
    };
    for key in keys {
        set.insert(key, json!({}));
    }
    Value::Object(set)
}

/// A port with no protocol is TCP, the way `EXPOSE 8080` means `8080/tcp`.
fn with_protocol(port: &String) -> String {
    if port.contains('/') {
        port.clone()
    } else {
        format!("{port}/tcp")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::containerfile::tar_layer::tar_layer;
    use crate::containerfile::upper::{Change, ChangeSet};

    pub(crate) fn layer() -> LayerBlob {
        tar_layer(&ChangeSet {
            changes: vec![Change::Regular {
                path: "spike-created".into(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                bytes: b"built-by-lns\n".to_vec(),
            }],
        })
        .unwrap()
    }

    pub(crate) fn parent_config() -> String {
        serde_json::json!({
            "architecture": "arm64",
            "os": "linux",
            "config": {
                "Env": ["PATH=/usr/bin"],
                "Cmd": ["/bin/sh"],
                "User": "app",
                "WorkingDir": "/srv",
            },
            "rootfs": { "type": "layers", "diff_ids": ["sha256:base"] },
            "history": [{ "created_by": "ADD alpine.tar" }],
        })
        .to_string()
    }

    pub(crate) fn parent_manifest() -> OciImageManifest {
        OciImageManifest {
            config: OciDescriptor {
                media_type: "application/vnd.oci.image.config.v1+json".into(),
                digest: "sha256:parentconfig".into(),
                size: 11,
                ..Default::default()
            },
            layers: vec![OciDescriptor {
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
                digest: "sha256:baseblob".into(),
                size: 42,
                ..Default::default()
            }],
            media_type: Some("application/vnd.oci.image.manifest.v1+json".into()),
            ..Default::default()
        }
    }

    pub(crate) fn parent() -> ParentImage {
        ParentImage {
            manifest: parent_manifest(),
            config: parent_config(),
        }
    }

    fn built() -> BuiltImage {
        assemble(
            &parent(),
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "2026-09-09T00:00:00Z",
        )
        .unwrap()
    }

    fn config_value(built: &BuiltImage) -> Value {
        serde_json::from_str(&built.config).unwrap()
    }

    #[test]
    fn the_new_layer_is_appended_to_the_parents_diff_ids_in_order() {
        let built = built();
        assert_eq!(
            config_value(&built)["rootfs"]["diff_ids"],
            json!(["sha256:base", layer().digest]),
        );
        assert_eq!(config_value(&built)["rootfs"]["type"], json!("layers"));
    }

    #[test]
    fn the_parents_runtime_configuration_is_carried_over_untouched() {
        let built = built();
        let config = config_value(&built);
        assert_eq!(config["architecture"], json!("arm64"));
        assert_eq!(config["os"], json!("linux"));
        assert_eq!(config["config"]["Env"], json!(["PATH=/usr/bin"]));
        assert_eq!(config["config"]["Cmd"], json!(["/bin/sh"]));
        assert_eq!(config["config"]["User"], json!("app"));
        assert_eq!(config["config"]["WorkingDir"], json!("/srv"));
    }

    #[test]
    fn one_history_entry_records_what_produced_the_layer() {
        let built = built();
        let history = config_value(&built)["history"].clone();
        assert_eq!(
            history,
            json!([
                { "created_by": "ADD alpine.tar" },
                { "created": "2026-09-09T00:00:00Z", "created_by": "RUN spike" },
            ]),
        );
        assert_eq!(
            config_value(&built)["created"],
            json!("2026-09-09T00:00:00Z")
        );
    }

    #[test]
    fn a_config_with_no_history_of_its_own_gets_one() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: json!({
                "rootfs": { "type": "layers", "diff_ids": ["sha256:base"] }
            })
            .to_string(),
        };
        let built = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap();
        assert_eq!(
            config_value(&built)["history"],
            json!([{ "created": "now", "created_by": "RUN spike" }]),
        );
    }

    #[test]
    fn the_manifest_appends_the_layer_as_an_uncompressed_tar_descriptor() {
        let built = built();
        let layer = layer();
        assert_eq!(built.manifest.layers.len(), 2);
        assert_eq!(
            built.manifest.layers[0].digest,
            parent_manifest().layers[0].digest
        );
        assert_eq!(
            built.manifest.layers[0].media_type,
            parent_manifest().layers[0].media_type,
        );
        assert_eq!(built.manifest.layers[1].digest, layer.digest);
        assert_eq!(built.manifest.layers[1].size, layer.size() as i64);
        assert_eq!(
            built.manifest.layers[1].media_type,
            "application/vnd.oci.image.layer.v1.tar",
        );
    }

    #[test]
    fn the_config_descriptor_points_at_the_config_this_build_wrote() {
        let built = built();
        assert_eq!(
            built.manifest.config.digest,
            format!(
                "sha256:{}",
                hex::encode(Sha256::digest(built.config.as_bytes()))
            ),
        );
        assert_eq!(built.manifest.config.size, built.config.len() as i64);
        assert_eq!(
            built.manifest.config.media_type, "application/vnd.oci.image.config.v1+json",
            "the parent's config media type decides how the config is read back",
        );
    }

    #[test]
    fn the_manifest_digest_is_the_sha256_of_the_manifest_as_it_is_stored() {
        let built = built();
        let bytes = serde_json::to_vec(&built.manifest).unwrap();
        assert_eq!(
            built.manifest_digest,
            format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
        );
    }

    #[test]
    fn a_built_image_is_never_an_artifact() {
        let mut parent = parent();
        parent.manifest.artifact_type = Some("application/vnd.lns.sandbox.v1+json".into());
        let built = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap();
        assert_eq!(built.manifest.artifact_type, None);
    }

    #[test]
    fn the_layer_count_and_the_diff_id_count_stay_equal() {
        let built = built();
        let diff_ids = config_value(&built)["rootfs"]["diff_ids"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(
            diff_ids,
            built.manifest.layers.len(),
            "a pull refuses an image whose counts disagree",
        );
    }

    fn drafted(draft: &ConfigDraft) -> Value {
        let built = assemble(&parent(), Some(&layer()), draft, "RUN one", "now").unwrap();
        serde_json::from_str::<Value>(&built.config).unwrap()["config"].clone()
    }

    #[test]
    fn a_declared_env_replaces_the_base_s_value_and_keeps_the_rest() {
        let draft = ConfigDraft {
            env: vec![
                ("PATH".into(), "/opt/bin".into()),
                ("MODE".into(), "research".into()),
            ],
            ..ConfigDraft::default()
        };
        assert_eq!(
            drafted(&draft)["Env"],
            json!(["PATH=/opt/bin", "MODE=research"]),
        );
    }

    #[test]
    fn a_declared_label_lands_beside_the_base_s_own() {
        let mut parent = parent();
        parent.config = json!({
            "config": { "Labels": { "org.opencontainers.image.vendor": "base" } },
            "rootfs": { "type": "layers", "diff_ids": ["sha256:base"] },
        })
        .to_string();
        let draft = ConfigDraft {
            labels: vec![("org.opencontainers.image.title".into(), "agent".into())],
            ..ConfigDraft::default()
        };
        let built = assemble(&parent, Some(&layer()), &draft, "LABEL one", "now").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&built.config).unwrap()["config"]["Labels"],
            json!({
                "org.opencontainers.image.vendor": "base",
                "org.opencontainers.image.title": "agent",
            }),
        );
    }

    #[test]
    fn a_declared_label_is_the_whole_set_where_the_base_declared_none() {
        let draft = ConfigDraft {
            labels: vec![("org.opencontainers.image.title".into(), "agent".into())],
            ..ConfigDraft::default()
        };
        assert_eq!(
            drafted(&draft)["Labels"],
            json!({ "org.opencontainers.image.title": "agent" }),
        );
    }

    #[test]
    fn a_declared_user_and_workdir_replace_the_base_s() {
        let draft = ConfigDraft {
            user: Some("node".into()),
            workdir: Some("/srv".into()),
            ..ConfigDraft::default()
        };
        let config = drafted(&draft);
        assert_eq!(config["User"], json!("node"));
        assert_eq!(config["WorkingDir"], json!("/srv"));
    }

    /// Docker's rule: an ENTRYPOINT with no CMD beside it drops whatever CMD the base image had,
    /// so the built image does not append the base's arguments to a different program.
    #[test]
    fn an_entrypoint_with_no_cmd_of_its_own_drops_the_base_s_cmd() {
        let draft = ConfigDraft {
            entrypoint: Some(vec!["/bin/agent".into()]),
            ..ConfigDraft::default()
        };
        let config = drafted(&draft);
        assert_eq!(config["Entrypoint"], json!(["/bin/agent"]));
        assert_eq!(config["Cmd"], Value::Null);
    }

    #[test]
    fn an_entrypoint_with_a_cmd_beside_it_keeps_both() {
        let draft = ConfigDraft {
            entrypoint: Some(vec!["/bin/agent".into()]),
            cmd: Some(vec!["--serve".into()]),
            ..ConfigDraft::default()
        };
        let config = drafted(&draft);
        assert_eq!(config["Entrypoint"], json!(["/bin/agent"]));
        assert_eq!(config["Cmd"], json!(["--serve"]));
    }

    #[test]
    fn a_declared_shell_lands_in_the_config_the_way_docker_records_it() {
        let draft = ConfigDraft {
            shell: Some(vec!["/bin/bash".into(), "-c".into()]),
            ..ConfigDraft::default()
        };
        assert_eq!(drafted(&draft)["Shell"], json!(["/bin/bash", "-c"]));
    }

    #[test]
    fn an_exposed_port_is_a_set_key_and_a_port_with_no_protocol_is_tcp() {
        let draft = ConfigDraft {
            exposed_ports: vec!["8080".into(), "5353/udp".into()],
            ..ConfigDraft::default()
        };
        assert_eq!(
            drafted(&draft)["ExposedPorts"],
            json!({ "8080/tcp": {}, "5353/udp": {} }),
        );
    }

    #[test]
    fn a_declared_volume_is_a_set_key_beside_the_base_s_own() {
        let mut parent = parent();
        parent.config = json!({
            "config": { "Volumes": { "/var/lib/base": {} } },
            "rootfs": { "type": "layers", "diff_ids": ["sha256:base"] },
        })
        .to_string();
        let draft = ConfigDraft {
            volumes: vec!["/data".into()],
            ..ConfigDraft::default()
        };
        let built = assemble(&parent, Some(&layer()), &draft, "VOLUME /data", "now").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&built.config).unwrap()["config"]["Volumes"],
            json!({ "/var/lib/base": {}, "/data": {} }),
        );
    }

    #[test]
    fn a_base_that_declares_no_config_block_gets_one() {
        let mut parent = parent();
        parent.config = json!({ "rootfs": { "type": "layers", "diff_ids": [] } }).to_string();
        let draft = ConfigDraft {
            user: Some("node".into()),
            ..ConfigDraft::default()
        };
        let built = assemble(&parent, Some(&layer()), &draft, "USER node", "now").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&built.config).unwrap()["config"]["User"],
            json!("node"),
        );
    }

    #[test]
    fn a_base_whose_config_block_is_not_an_object_is_refused_by_name() {
        let mut parent = parent();
        parent.config = json!({
            "config": "nope",
            "rootfs": { "type": "layers", "diff_ids": [] },
        })
        .to_string();
        let err = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN one",
            "now",
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("config block that is not an object"),
            "{err:#}"
        );
    }

    /// An instruction that writes only config adds no layer and no diff id, and says so in the
    /// history the way the classic builder does.
    #[test]
    fn a_config_only_commit_adds_a_history_entry_and_no_layer() {
        let draft = ConfigDraft {
            env: vec![("MODE".into(), "research".into())],
            ..ConfigDraft::default()
        };
        let built = assemble(&parent(), None, &draft, "ENV MODE=research", "now").unwrap();
        let config: Value = serde_json::from_str(&built.config).unwrap();

        assert_eq!(
            built.manifest.layers.len(),
            parent_manifest().layers.len(),
            "a config-only commit appends no layer descriptor",
        );
        assert_eq!(config["rootfs"]["diff_ids"], json!(["sha256:base"]));
        assert_eq!(
            config["history"].as_array().unwrap().last().unwrap(),
            &json!({
                "created": "now",
                "created_by": "ENV MODE=research",
                "empty_layer": true,
            }),
        );
    }

    #[test]
    fn a_config_that_is_not_json_names_the_base_image() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: "not json".into(),
        };
        let err = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing the base image's config"),
            "{err:#}"
        );
    }

    #[test]
    fn a_config_that_is_not_an_object_is_refused() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: "[]".into(),
        };
        let err = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a JSON object"), "{err:#}");
    }

    #[test]
    fn a_config_without_diff_ids_is_refused_by_name() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: json!({ "rootfs": { "type": "layers" } }).to_string(),
        };
        let err = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no rootfs.diff_ids"), "{err:#}");
    }

    #[test]
    fn a_config_whose_history_is_not_a_list_is_refused() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: json!({
                "rootfs": { "type": "layers", "diff_ids": [] },
                "history": "nope",
            })
            .to_string(),
        };
        let err = assemble(
            &parent,
            Some(&layer()),
            &ConfigDraft::default(),
            "RUN spike",
            "now",
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("history that is not a list"),
            "{err:#}"
        );
    }
}
