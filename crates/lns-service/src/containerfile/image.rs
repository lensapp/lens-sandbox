use anyhow::{Context, Result, bail};
use oci_client::manifest::{OciDescriptor, OciImageManifest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
    layer: &LayerBlob,
    created_by: &str,
    created: &str,
) -> Result<BuiltImage> {
    let config = config_over(&parent.config, layer, created_by, created)?;
    let config_digest = format!("sha256:{}", hex::encode(Sha256::digest(config.as_bytes())));

    let mut manifest = parent.manifest.clone();
    manifest.config = OciDescriptor {
        media_type: parent.manifest.config.media_type.clone(),
        digest: config_digest,
        size: config.len() as i64,
        ..Default::default()
    };
    manifest.layers.push(OciDescriptor {
        media_type: LAYER_MEDIA_TYPE.to_string(),
        digest: layer.digest.clone(),
        size: layer.size() as i64,
        ..Default::default()
    });
    manifest.artifact_type = None;

    let manifest_bytes = serde_json::to_vec(&manifest).context("serializing the built manifest")?;
    Ok(BuiltImage {
        manifest,
        manifest_digest: format!("sha256:{}", hex::encode(Sha256::digest(&manifest_bytes))),
        config,
    })
}

fn config_over(parent: &str, layer: &LayerBlob, created_by: &str, created: &str) -> Result<String> {
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
    diff_ids.push(json!(layer.digest));

    let history = object
        .entry("history".to_string())
        .or_insert_with(|| json!([]));
    let Some(history) = history.as_array_mut() else {
        bail!("the base image's config has a history that is not a list");
    };
    history.push(json!({ "created": created, "created_by": created_by }));

    serde_json::to_string(&config).context("serializing the built config")
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
        assemble(&parent(), &layer(), "RUN spike", "2026-09-09T00:00:00Z").unwrap()
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
        let built = assemble(&parent, &layer(), "RUN spike", "now").unwrap();
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
        let built = assemble(&parent, &layer(), "RUN spike", "now").unwrap();
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

    #[test]
    fn a_config_that_is_not_json_names_the_base_image() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: "not json".into(),
        };
        let err = assemble(&parent, &layer(), "RUN spike", "now").unwrap_err();
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
        let err = assemble(&parent, &layer(), "RUN spike", "now").unwrap_err();
        assert!(format!("{err:#}").contains("not a JSON object"), "{err:#}");
    }

    #[test]
    fn a_config_without_diff_ids_is_refused_by_name() {
        let parent = ParentImage {
            manifest: parent_manifest(),
            config: json!({ "rootfs": { "type": "layers" } }).to_string(),
        };
        let err = assemble(&parent, &layer(), "RUN spike", "now").unwrap_err();
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
        let err = assemble(&parent, &layer(), "RUN spike", "now").unwrap_err();
        assert!(
            format!("{err:#}").contains("history that is not a list"),
            "{err:#}"
        );
    }
}
