use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_policy::artifact::{self, Family};

use crate::cli::{PullArgs, PushArgs};
use crate::command::{CommandSpec, subcommand};
use crate::integration::LocalBoxFuture;

mod real;
pub use real::RealRegistryClient;

pub fn augment_push(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<PushArgs>("push")
            .about("Push a file (typed artifact) or a cached image to an OCI registry reference."),
    )
}

pub const PUSH_SPEC: CommandSpec = CommandSpec {
    name: "push",
    augment: augment_push,
    run: real::run_push,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn augment_pull(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<PullArgs>("pull")
            .about("Pull an artifact or image from an OCI registry reference."),
    )
}

pub const PULL_SPEC: CommandSpec = CommandSpec {
    name: "pull",
    augment: augment_pull,
    run: real::run_pull,
    announces_update_check: true,
    owns_terminal: false,
};

pub enum Pulled {
    Artifact {
        artifact_type: String,
        config_blob: Vec<u8>,
        digest: String,
    },
    Image {
        digest: String,
    },
}

/// Pushes/pulls artifacts and images through the running service, which owns the OCI registry client.
pub trait RegistryClient {
    fn push_artifact<'a>(
        &'a self,
        reference: &'a str,
        artifact_type: &'a str,
        config_media_type: &'a str,
        config_blob: &'a [u8],
        layers: &'a [Vec<u8>],
    ) -> LocalBoxFuture<'a, Result<String>>;

    fn push_image<'a>(
        &'a self,
        source_reference: &'a str,
        target_reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>>;

    fn pull<'a>(&'a self, reference: &'a str) -> LocalBoxFuture<'a, Result<Pulled>>;
}

fn resolve_family(explicit: Option<&str>, reference: &str) -> Result<Family> {
    if let Some(slug) = explicit {
        return Family::from_slug(slug).ok_or_else(|| {
            let known: Vec<&str> = Family::ALL.iter().map(|f| f.slug()).collect();
            anyhow::anyhow!(
                "unknown artifact family {slug:?}; expected one of {}",
                known.join(", ")
            )
        });
    }
    Family::infer_from_reference(reference).ok_or_else(|| {
        anyhow::anyhow!(
            "could not infer the artifact family from {reference:?}; pass --family (e.g. --family agent)"
        )
    })
}

pub async fn push(
    args: &PushArgs,
    cwd: &Path,
    client: &dyn RegistryClient,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = if Path::new(&args.source).is_absolute() {
        PathBuf::from(&args.source)
    } else {
        cwd.join(&args.source)
    };
    if !path.exists() {
        let digest = client.push_image(&args.source, &args.reference).await?;
        writeln!(writer, "Pushed image {} to {}", digest, args.reference)?;
        return Ok(0);
    }
    let family = resolve_family(args.family.as_deref(), &args.reference)?;
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let config_blob = artifact::to_config_blob(&bytes).with_context(|| {
        format!(
            "parsing {} as the {} artifact",
            path.display(),
            family.slug()
        )
    })?;
    let layers = match &args.content {
        Some(content) => {
            let content = if Path::new(content).is_absolute() {
                content.clone()
            } else {
                cwd.join(content)
            };
            vec![pack_content_layer(&content)?]
        }
        None => Vec::new(),
    };
    let digest = client
        .push_artifact(
            &args.reference,
            &family.artifact_type(),
            &family.config_media_type(),
            &config_blob,
            &layers,
        )
        .await?;
    writeln!(
        writer,
        "Pushed {} {} ({} bytes{}) to {}",
        family.slug(),
        digest,
        config_blob.len(),
        if layers.is_empty() {
            String::new()
        } else {
            format!(" + {} layer", layers.len())
        },
        args.reference
    )?;
    Ok(0)
}

/// Packs a file or directory tree into a single gzip-compressed tar layer (relative paths, rooted at the content dir).
fn pack_content_layer(content: &Path) -> Result<Vec<u8>> {
    use flate2::{Compression, write::GzEncoder};
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = tar::Builder::new(&mut gz);
        if content.is_dir() {
            builder
                .append_dir_all(".", content)
                .with_context(|| format!("packing directory {}", content.display()))?;
        } else {
            let name = content.file_name().ok_or_else(|| {
                anyhow::anyhow!("content path has no file name: {}", content.display())
            })?;
            let mut file = std::fs::File::open(content)
                .with_context(|| format!("opening {}", content.display()))?;
            builder
                .append_file(name, &mut file)
                .with_context(|| format!("packing file {}", content.display()))?;
        }
        builder.finish().context("finalizing content tar")?;
    }
    gz.finish().context("finalizing content gzip")
}

pub async fn pull(
    args: &PullArgs,
    client: &dyn RegistryClient,
    writer: &mut impl Write,
) -> Result<i32> {
    match client.pull(&args.reference).await? {
        Pulled::Artifact {
            artifact_type,
            config_blob,
            digest,
        } => match &args.output {
            Some(path) => {
                std::fs::write(path, &config_blob)
                    .with_context(|| format!("writing artifact to {}", path.display()))?;
                writeln!(
                    writer,
                    "Pulled {artifact_type} {digest} to {}",
                    path.display()
                )?;
            }
            None => writer.write_all(&config_blob)?,
        },
        Pulled::Image { digest } => {
            writeln!(writer, "Pulled image {digest} into the cache")?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn push_and_pull_specs_register_under_their_verbs() {
        assert_eq!(PUSH_SPEC.name, "push");
        assert_eq!(PULL_SPEC.name, "pull");
        // augment must wire each verb's subcommand into the CLI.
        augment_push(clap::Command::new("lns"));
        augment_pull(clap::Command::new("lns"));
    }

    #[derive(Default)]
    struct FakeClient {
        pushed: Mutex<Option<(String, String, Vec<u8>)>>,
        pushed_layers: Mutex<Vec<Vec<u8>>>,
        image_pushed: Mutex<Option<(String, String)>>,
        push_digest: String,
        pull: Option<Pulled>,
        fail: Option<String>,
    }

    impl RegistryClient for FakeClient {
        fn push_image<'a>(
            &'a self,
            source_reference: &'a str,
            target_reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                *self.image_pushed.lock().unwrap() =
                    Some((source_reference.into(), target_reference.into()));
                Ok(self.push_digest.clone())
            })
        }

        fn push_artifact<'a>(
            &'a self,
            reference: &'a str,
            artifact_type: &'a str,
            _config_media_type: &'a str,
            config_blob: &'a [u8],
            layers: &'a [Vec<u8>],
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                *self.pushed.lock().unwrap() =
                    Some((reference.into(), artifact_type.into(), config_blob.to_vec()));
                *self.pushed_layers.lock().unwrap() = layers.to_vec();
                Ok(self.push_digest.clone())
            })
        }

        fn pull<'a>(&'a self, _reference: &'a str) -> LocalBoxFuture<'a, Result<Pulled>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                match self.pull.as_ref().expect("canned pull") {
                    Pulled::Artifact {
                        artifact_type,
                        config_blob,
                        digest,
                    } => Ok(Pulled::Artifact {
                        artifact_type: artifact_type.clone(),
                        config_blob: config_blob.clone(),
                        digest: digest.clone(),
                    }),
                    Pulled::Image { digest } => Ok(Pulled::Image {
                        digest: digest.clone(),
                    }),
                }
            })
        }
    }

    fn push_args(source: &str, reference: &str, family: Option<&str>) -> PushArgs {
        PushArgs {
            source: source.into(),
            reference: reference.into(),
            family: family.map(str::to_string),
            content: None,
        }
    }

    #[test]
    fn resolve_family_uses_explicit_then_reference_then_errors() {
        assert_eq!(resolve_family(Some("agent"), "x").unwrap(), Family::Agent);
        assert_eq!(
            resolve_family(None, "localhost:5000/org/acme/policies/pii:v1").unwrap(),
            Family::Policy
        );
        assert!(
            format!("{:#}", resolve_family(Some("ghost"), "x").unwrap_err())
                .contains("unknown artifact family")
        );
        assert!(
            format!(
                "{:#}",
                resolve_family(None, "docker.io/library/alpine:3.20").unwrap_err()
            )
            .contains("could not infer")
        );
    }

    #[tokio::test]
    async fn push_infers_family_from_the_reference_and_uploads_the_file_as_a_blob() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("agent.yaml"),
            "apiVersion: lens.dev/v1alpha1\n",
        )
        .unwrap();
        let client = FakeClient {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let args = push_args(
            "agent.yaml",
            "localhost:5000/org/acme/agents/hermes:v1",
            None,
        );
        let mut out = Vec::new();
        assert_eq!(push(&args, dir.path(), &client, &mut out).await.unwrap(), 0);
        let (reference, artifact_type, blob) = client.pushed.lock().unwrap().clone().unwrap();
        assert_eq!(reference, "localhost:5000/org/acme/agents/hermes:v1");
        assert_eq!(artifact_type, "application/vnd.lens.agent.v1+json");
        let v: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(v["apiVersion"], "lens.dev/v1alpha1");
        assert!(String::from_utf8(out).unwrap().contains("sha256:abc"));
    }

    #[tokio::test]
    async fn push_accepts_an_absolute_source_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("p.yaml");
        std::fs::write(&file, "network: {}\n").unwrap();
        let client = FakeClient {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let args = push_args(
            file.to_str().unwrap(),
            "localhost:5000/org/acme/policies/p:v1",
            None,
        );
        // cwd is irrelevant for an absolute source.
        let other = TempDir::new().unwrap();
        assert_eq!(
            push(&args, other.path(), &client, &mut Vec::new())
                .await
                .unwrap(),
            0
        );
        assert!(client.pushed.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn push_surfaces_a_malformed_artifact_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("p.yaml"), ": : not yaml : :").unwrap();
        let err = push(
            &push_args("p.yaml", "localhost:5000/org/acme/policies/p:v1", None),
            dir.path(),
            &FakeClient::default(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("as the policy artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_honours_an_explicit_family_override() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("p.yaml"), "network: {}\n").unwrap();
        let client = FakeClient {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let args = push_args("p.yaml", "localhost:5000/whatever:v1", Some("policy"));
        push(&args, dir.path(), &client, &mut Vec::new())
            .await
            .unwrap();
        let (_, artifact_type, _) = client.pushed.lock().unwrap().clone().unwrap();
        assert_eq!(artifact_type, "application/vnd.lens.policy.v1+json");
    }

    #[tokio::test]
    async fn push_errors_when_the_family_cannot_be_determined() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("f.yaml"), "x: 1\n").unwrap();
        let err = push(
            &push_args("f.yaml", "localhost:5000/just-a-name:v1", None),
            dir.path(),
            &FakeClient::default(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("could not infer"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_treats_a_non_file_source_as_an_image_push() {
        let dir = TempDir::new().unwrap();
        let client = FakeClient {
            push_digest: "sha256:img".into(),
            ..Default::default()
        };
        let args = push_args(
            "docker.io/library/alpine:3.20",
            "localhost:5000/org/x/images/alpine:3.20",
            None,
        );
        let mut out = Vec::new();
        assert_eq!(push(&args, dir.path(), &client, &mut out).await.unwrap(), 0);
        assert_eq!(
            client.image_pushed.lock().unwrap().clone().unwrap(),
            (
                "docker.io/library/alpine:3.20".to_string(),
                "localhost:5000/org/x/images/alpine:3.20".to_string()
            )
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Pushed image sha256:img")
        );
    }

    #[tokio::test]
    async fn push_image_surfaces_a_registry_error() {
        let dir = TempDir::new().unwrap();
        let client = FakeClient {
            fail: Some("registry said no".into()),
            ..Default::default()
        };
        let err = push(
            &push_args(
                "docker.io/library/alpine:3.20",
                "localhost:5000/org/x/images/alpine:3.20",
                None,
            ),
            dir.path(),
            &client,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("registry said no"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_surfaces_a_registry_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("p.yaml"), "network: {}\n").unwrap();
        let client = FakeClient {
            fail: Some("registry said no".into()),
            ..Default::default()
        };
        let err = push(
            &push_args("p.yaml", "localhost:5000/org/x/policies/p:v1", None),
            dir.path(),
            &client,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("registry said no"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_writes_an_artifact_to_the_output_file() {
        let dir = TempDir::new().unwrap();
        let out_path = dir.path().join("pulled.json");
        let client = FakeClient {
            pull: Some(Pulled::Artifact {
                artifact_type: "application/vnd.lens.agent.v1+json".into(),
                config_blob: br#"{"kind":"Agent"}"#.to_vec(),
                digest: "sha256:def".into(),
            }),
            ..Default::default()
        };
        let args = PullArgs {
            reference: "localhost:5000/org/x/agents/h:v1".into(),
            output: Some(out_path.clone()),
        };
        let mut out = Vec::new();
        pull(&args, &client, &mut out).await.unwrap();
        assert_eq!(std::fs::read(&out_path).unwrap(), br#"{"kind":"Agent"}"#);
        assert!(String::from_utf8(out).unwrap().contains("sha256:def"));
    }

    #[tokio::test]
    async fn pull_writes_an_artifact_to_stdout_when_no_output() {
        let client = FakeClient {
            pull: Some(Pulled::Artifact {
                artifact_type: "application/vnd.lens.policy.v1+json".into(),
                config_blob: br#"{"network":{}}"#.to_vec(),
                digest: "sha256:def".into(),
            }),
            ..Default::default()
        };
        let args = PullArgs {
            reference: "localhost:5000/org/x/policies/p:v1".into(),
            output: None,
        };
        let mut out = Vec::new();
        pull(&args, &client, &mut out).await.unwrap();
        assert_eq!(out, br#"{"network":{}}"#);
    }

    #[tokio::test]
    async fn pull_reports_an_image_pulled_into_the_cache() {
        let client = FakeClient {
            pull: Some(Pulled::Image {
                digest: "sha256:img".into(),
            }),
            ..Default::default()
        };
        let args = PullArgs {
            reference: "localhost:5000/org/x/images/alpine:3.20".into(),
            output: None,
        };
        let mut out = Vec::new();
        pull(&args, &client, &mut out).await.unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Pulled image sha256:img")
        );
    }

    #[tokio::test]
    async fn pull_surfaces_a_registry_error() {
        let client = FakeClient {
            fail: Some("not found".into()),
            ..Default::default()
        };
        let args = PullArgs {
            reference: "localhost:5000/org/x/agents/h:v1".into(),
            output: None,
        };
        let err = pull(&args, &client, &mut Vec::new()).await.unwrap_err();
        assert!(format!("{err:#}").contains("not found"), "got: {err:#}");
    }

    fn read_gz_tar_names(layer: &[u8]) -> Vec<String> {
        let dec = flate2::read::GzDecoder::new(layer);
        let mut archive = tar::Archive::new(dec);
        archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn pack_content_layer_packs_a_single_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.yaml"), b"model: {}\n").unwrap();
        let layer = pack_content_layer(&dir.path().join("config.yaml")).unwrap();
        assert!(
            read_gz_tar_names(&layer)
                .iter()
                .any(|n| n.ends_with("config.yaml"))
        );
    }

    #[test]
    fn pack_content_layer_rejects_a_path_with_no_file_name() {
        let err = pack_content_layer(Path::new("does-not-exist/..")).unwrap_err();
        assert!(format!("{err:#}").contains("no file name"), "got: {err:#}");
    }

    #[test]
    fn pack_content_layer_packs_a_directory_tree() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.yaml"), b"x: 1\n").unwrap();
        let layer = pack_content_layer(dir.path()).unwrap();
        assert!(
            read_gz_tar_names(&layer)
                .iter()
                .any(|n| n.contains("config.yaml"))
        );
    }

    #[tokio::test]
    async fn push_packs_content_into_a_layer_and_forwards_it() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("fs.yaml"),
            "apiVersion: lens.dev/v1alpha1\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("payload")).unwrap();
        std::fs::write(dir.path().join("payload/config.yaml"), b"model: {}\n").unwrap();
        let client = FakeClient {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let mut args = push_args("fs.yaml", "localhost:5000/org/acme/filesets/cfg:v1", None);
        args.content = Some("payload".into());
        let mut out = Vec::new();
        assert_eq!(push(&args, dir.path(), &client, &mut out).await.unwrap(), 0);
        let layers = client.pushed_layers.lock().unwrap().clone();
        assert_eq!(layers.len(), 1, "one content layer pushed");
        assert!(
            read_gz_tar_names(&layers[0])
                .iter()
                .any(|n| n.contains("config.yaml")),
            "layer carries the packed content"
        );
        assert!(String::from_utf8(out).unwrap().contains("1 layer"));
    }

    #[tokio::test]
    async fn push_packs_an_absolute_content_path() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("fs.yaml"),
            "apiVersion: lens.dev/v1alpha1\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("payload")).unwrap();
        std::fs::write(dir.path().join("payload/config.yaml"), b"model: {}\n").unwrap();
        let client = FakeClient {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let mut args = push_args("fs.yaml", "localhost:5000/org/acme/filesets/cfg:v1", None);
        args.content = Some(dir.path().join("payload"));
        let mut out = Vec::new();
        assert_eq!(push(&args, dir.path(), &client, &mut out).await.unwrap(), 0);
        let layers = client.pushed_layers.lock().unwrap().clone();
        assert_eq!(layers.len(), 1, "one content layer pushed");
        assert!(
            read_gz_tar_names(&layers[0])
                .iter()
                .any(|n| n.contains("config.yaml")),
            "absolute content path packed"
        );
    }
}
