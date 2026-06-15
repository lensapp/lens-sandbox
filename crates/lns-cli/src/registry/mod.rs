use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_policy::artifact::{self, Family};

use crate::cli::{PullArgs, PushArgs};
use crate::integration::LocalBoxFuture;

mod real;
pub use real::RealRegistryClient;

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
        bail!(
            "image push is not yet supported; {:?} is not a local file to push as an artifact",
            args.source
        );
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
    let digest = client
        .push_artifact(
            &args.reference,
            &family.artifact_type(),
            &family.config_media_type(),
            &config_blob,
        )
        .await?;
    writeln!(
        writer,
        "Pushed {} {} ({} bytes) to {}",
        family.slug(),
        digest,
        config_blob.len(),
        args.reference
    )?;
    Ok(0)
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[derive(Default)]
    struct FakeClient {
        pushed: Mutex<Option<(String, String, Vec<u8>)>>,
        push_digest: String,
        pull: Option<Pulled>,
        fail: Option<String>,
    }

    impl RegistryClient for FakeClient {
        fn push_artifact<'a>(
            &'a self,
            reference: &'a str,
            artifact_type: &'a str,
            _config_media_type: &'a str,
            config_blob: &'a [u8],
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                *self.pushed.lock().unwrap() =
                    Some((reference.into(), artifact_type.into(), config_blob.to_vec()));
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
    async fn push_rejects_a_non_file_source_as_unsupported_image_push() {
        let dir = TempDir::new().unwrap();
        let err = push(
            &push_args(
                "localhost:5000/org/x/images/alpine:3.20",
                "localhost:5000/org/x/y:v1",
                None,
            ),
            dir.path(),
            &FakeClient::default(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("image push is not yet supported"),
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
}
