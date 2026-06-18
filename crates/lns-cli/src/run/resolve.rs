use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_policy::artifact::{AgentArtifact, BundleArtifact, CredentialRef, Family};
use lns_policy::credentials::{CredentialEntry, CredentialStateFile};

use crate::cli::RunArgs;
use crate::registry::{Pulled, RegistryClient};

/// A registry artifact reference resolved into the concrete inputs `lns run` needs.
#[derive(Debug)]
pub struct ResolvedRun {
    pub image: String,
    pub cmd: Vec<String>,
    pub cpus: Option<u8>,
    pub mem: Option<usize>,
    pub sandbox_user: Option<String>,
    pub ports: Vec<lns_ipc::PortPublish>,
    pub volumes: Vec<lns_ipc::VolumeMount>,
    pub policy_path: Option<PathBuf>,
    pub credentials: Vec<CredentialRef>,
    pub _policy_tempfile: Option<tempfile::NamedTempFile>,
}

/// Resolves an agent/bundle `image` reference into a concrete `RunArgs`, warning about unprovisioned credentials; a plain image or imageless run passes through untouched. The returned tempfile (if any) backs an ephemeral materialized policy and must outlive the run.
pub async fn resolve_into_run_args(
    mut args: RunArgs,
    client: &dyn RegistryClient,
    available_creds: &[String],
    writer: &mut impl Write,
) -> Result<(RunArgs, Option<tempfile::NamedTempFile>)> {
    let Some(reference) = args.image.clone() else {
        return Ok((args, None));
    };
    let resolved = match Family::infer_from_reference(&reference) {
        Some(Family::Agent) => resolve_agent_ref(&reference, client, writer).await?,
        Some(Family::Bundle) => resolve_bundle_ref(&reference, client, writer).await?,
        Some(other) => bail!(
            "{} references cannot be run directly — pass an image, agent, or bundle reference",
            other.slug()
        ),
        None => return Ok((args, None)),
    };
    args.image = Some(resolved.image);
    if args.cmd.is_empty() {
        args.cmd = resolved.cmd;
    }
    if args.cpus.is_none() {
        args.cpus = resolved.cpus;
    }
    if args.mem.is_none() {
        args.mem = resolved.mem;
    }
    if args.sandbox_user.is_none() {
        args.sandbox_user = resolved.sandbox_user;
    }
    if args.publish.is_empty() {
        args.publish = resolved.ports;
    }
    if args.volumes.is_empty() {
        args.volumes = resolved.volumes;
    }
    if args.policy.is_none()
        && let Some(p) = &resolved.policy_path
    {
        args.policy = Some(p.clone());
    }
    let missing = missing_credentials(&resolved.credentials, available_creds);
    if !missing.is_empty() {
        writeln!(writer, "⚠ {}", missing_credentials_warning(&missing))?;
    }
    Ok((args, resolved._policy_tempfile))
}

/// The credential names with a usable (non-`Deny`) entry in the local credential store.
pub fn available_credentials(state: &CredentialStateFile) -> Vec<String> {
    state
        .iter()
        .filter(|(_, entry)| **entry != CredentialEntry::Deny)
        .map(|(name, _)| name.clone())
        .collect()
}

pub async fn resolve_agent_ref(
    reference: &str,
    client: &dyn RegistryClient,
    writer: &mut impl Write,
) -> Result<ResolvedRun> {
    let agent = pull_agent(reference, client).await?;
    writeln!(
        writer,
        "✓ resolved agent {} → {}",
        agent.metadata.name, agent.spec.image
    )?;
    let (cpus, mem) = match &agent.spec.resources {
        Some(r) => (
            r.cpus.and_then(|c| u8::try_from(c).ok()),
            r.memory_mib.map(|m| m as usize),
        ),
        None => (None, None),
    };
    Ok(ResolvedRun {
        cmd: command_argv(agent.spec.command.as_deref()),
        image: agent.spec.image,
        cpus,
        mem,
        sandbox_user: agent.spec.user,
        ports: agent.spec.ports.iter().map(port_publish).collect(),
        volumes: agent.spec.volumes.iter().map(volume_mount).collect(),
        policy_path: None,
        credentials: agent.spec.credentials,
        _policy_tempfile: None,
    })
}

fn port_publish(p: &lns_policy::artifact::PortMapping) -> lns_ipc::PortPublish {
    lns_ipc::PortPublish {
        host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        host_port: p.host,
        container_port: p.container,
        protocol: lns_ipc::Protocol::Tcp,
    }
}

fn volume_mount(v: &lns_policy::artifact::VolumeMapping) -> lns_ipc::VolumeMount {
    lns_ipc::VolumeMount {
        name: v.name.clone(),
        target: v.target.clone(),
        read_only: v.read_only,
    }
}

async fn pull_agent(reference: &str, client: &dyn RegistryClient) -> Result<AgentArtifact> {
    match client.pull(reference).await? {
        Pulled::Artifact {
            artifact_type,
            config_blob,
            ..
        } => {
            if Family::from_artifact_type(&artifact_type) != Some(Family::Agent) {
                bail!("{reference} is not an agent artifact (media type {artifact_type})");
            }
            AgentArtifact::from_config_blob(&config_blob)
                .with_context(|| format!("parsing agent artifact {reference}"))
        }
        Pulled::Image { .. } => {
            bail!("{reference} resolved to an image, not an agent artifact")
        }
    }
}

fn command_argv(command: Option<&str>) -> Vec<String> {
    match command {
        Some(c) if !c.trim().is_empty() => vec!["/bin/sh".into(), "-c".into(), c.to_string()],
        _ => Vec::new(),
    }
}

pub async fn resolve_bundle_ref(
    reference: &str,
    client: &dyn RegistryClient,
    writer: &mut impl Write,
) -> Result<ResolvedRun> {
    let bundle = pull_bundle(reference, client).await?;
    let agent_ref = single_agent_ref(reference, &bundle)?;
    let mut resolved = resolve_agent_ref(&agent_ref, client, writer).await?;
    if let Some(component) = bundle.components.policies.first() {
        let policy_ref = qualify_component_ref(reference, &component.reference);
        let (path, file) = materialize_policy(&policy_ref, client).await?;
        writeln!(writer, "✓ applied bundle policy {policy_ref}")?;
        resolved.policy_path = Some(path);
        resolved._policy_tempfile = Some(file);
    }
    note_skipped_components(&bundle, writer)?;
    Ok(resolved)
}

async fn pull_bundle(reference: &str, client: &dyn RegistryClient) -> Result<BundleArtifact> {
    match client.pull(reference).await? {
        Pulled::Artifact {
            artifact_type,
            config_blob,
            ..
        } => {
            if Family::from_artifact_type(&artifact_type) != Some(Family::Bundle) {
                bail!("{reference} is not a bundle artifact (media type {artifact_type})");
            }
            BundleArtifact::from_config_blob(&config_blob)
                .with_context(|| format!("parsing bundle artifact {reference}"))
        }
        Pulled::Image { .. } => {
            bail!("{reference} resolved to an image, not a bundle artifact")
        }
    }
}

fn single_agent_ref(bundle_ref: &str, bundle: &BundleArtifact) -> Result<String> {
    match bundle.components.agents.as_slice() {
        [] => bail!("bundle {bundle_ref} has no agent component to run"),
        [one] => Ok(qualify_component_ref(bundle_ref, &one.reference)),
        many => bail!(
            "bundle {bundle_ref} has {} agents; multi-agent bundles are not supported yet",
            many.len()
        ),
    }
}

async fn materialize_policy(
    policy_ref: &str,
    client: &dyn RegistryClient,
) -> Result<(PathBuf, tempfile::NamedTempFile)> {
    let blob = match client.pull(policy_ref).await? {
        Pulled::Artifact {
            artifact_type,
            config_blob,
            ..
        } => {
            if Family::from_artifact_type(&artifact_type) != Some(Family::Policy) {
                bail!("{policy_ref} is not a policy artifact (media type {artifact_type})");
            }
            config_blob
        }
        Pulled::Image { .. } => bail!("{policy_ref} resolved to an image, not a policy artifact"),
    };
    let mut file = tempfile::Builder::new()
        .prefix("lns-bundle-policy-")
        .suffix(".yaml")
        .tempfile()
        .context("creating a temp file for the bundle policy")?;
    file.write_all(&blob)
        .context("writing the materialized bundle policy")?;
    file.flush().ok();
    let path = file.path().to_path_buf();
    Ok((path, file))
}

fn note_skipped_components(bundle: &BundleArtifact, writer: &mut impl Write) -> Result<()> {
    let mut skipped = Vec::new();
    if !bundle.components.tools.is_empty() {
        skipped.push("tools");
    }
    if bundle.components.sandbox.is_some() {
        skipped.push("sandbox");
    }
    if !bundle.components.knowledge.is_empty() {
        skipped.push("knowledge");
    }
    if !skipped.is_empty() {
        writeln!(
            writer,
            "• bundle components not yet applied: {}",
            skipped.join(", ")
        )?;
    }
    Ok(())
}

fn registry_host(reference: &str) -> Option<&str> {
    let first = reference.split('/').next()?;
    (first.contains('.') || first.contains(':') || first == "localhost").then_some(first)
}

/// Prepends the bundle reference's registry host to a host-less component reference (already-qualified refs pass through).
pub fn qualify_component_ref(bundle_ref: &str, component_ref: &str) -> String {
    if registry_host(component_ref).is_some() {
        return component_ref.to_string();
    }
    match registry_host(bundle_ref) {
        Some(host) => format!("{host}/{component_ref}"),
        None => component_ref.to_string(),
    }
}

/// The agent's required credentials that have no usable entry in the local credential store — the run can still proceed (the boundary asks), so callers warn rather than abort.
pub fn missing_credentials(required: &[CredentialRef], available: &[String]) -> Vec<String> {
    required
        .iter()
        .map(|c| c.name.clone())
        .filter(|name| !available.iter().any(|a| a == name))
        .collect()
}

/// A one-line warning naming the unprovisioned credentials and how to connect them.
pub fn missing_credentials_warning(missing: &[String]) -> String {
    let hints = missing
        .iter()
        .map(|n| format!("`lns connect {n}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "agent credentials not set up: {} — the sandbox will ask at first use, or run {hints}",
        missing.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::LocalBoxFuture;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const AGENT_REF: &str = "localhost:5000/org/acme/agents/some-agent:v1";
    const POLICY_REF: &str = "localhost:5000/org/acme/policies/some-egress:v1";
    const BUNDLE_REF: &str = "localhost:5000/org/acme/bundles/some-system:v1";
    const AGENT_IMAGE: &str = "localhost:5000/org/acme/images/some-agent:v1";

    type ArtifactMap = Arc<Mutex<HashMap<String, (String, Vec<u8>)>>>;
    type ImageMap = Arc<Mutex<HashMap<String, String>>>;

    #[derive(Default, Clone)]
    struct RefKeyedClient {
        artifacts: ArtifactMap,
        images: ImageMap,
        fail: Option<String>,
    }

    impl RegistryClient for RefKeyedClient {
        fn push_artifact<'a>(
            &'a self,
            reference: &'a str,
            artifact_type: &'a str,
            _config_media_type: &'a str,
            config_blob: &'a [u8],
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.artifacts.lock().unwrap().insert(
                    reference.to_string(),
                    (artifact_type.to_string(), config_blob.to_vec()),
                );
                Ok(format!("sha256:{}", config_blob.len()))
            })
        }

        fn push_image<'a>(
            &'a self,
            _source: &'a str,
            target: &'a str,
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.images
                    .lock()
                    .unwrap()
                    .insert(target.to_string(), "sha256:img".to_string());
                Ok("sha256:img".to_string())
            })
        }

        fn pull<'a>(&'a self, reference: &'a str) -> LocalBoxFuture<'a, Result<Pulled>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                if let Some((artifact_type, config_blob)) =
                    self.artifacts.lock().unwrap().get(reference).cloned()
                {
                    return Ok(Pulled::Artifact {
                        artifact_type,
                        config_blob,
                        digest: "sha256:test".into(),
                    });
                }
                if let Some(digest) = self.images.lock().unwrap().get(reference).cloned() {
                    return Ok(Pulled::Image { digest });
                }
                bail!("no canned pull for {reference}")
            })
        }
    }

    async fn put_artifact(client: &RefKeyedClient, reference: &str, family: Family, blob: &[u8]) {
        client
            .push_artifact(
                reference,
                &family.artifact_type(),
                &family.config_media_type(),
                blob,
            )
            .await
            .unwrap();
    }

    async fn put_image(client: &RefKeyedClient, reference: &str) {
        client
            .push_image("source-image:1", reference)
            .await
            .unwrap();
    }

    fn agent_blob(command: &str, with_cred: bool) -> Vec<u8> {
        let creds = if with_cred {
            "  credentials:\n    - { name: some-provider, env: SOME_TOKEN }\n"
        } else {
            ""
        };
        let cmd = if command.is_empty() {
            String::new()
        } else {
            format!("  command: '{command}'\n")
        };
        lns_policy::artifact::to_config_blob(
            format!(
                "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
                 metadata:\n  name: some-agent\n\
                 spec:\n  image: {AGENT_IMAGE}\n{cmd}{creds}"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    fn bundle_blob(components: &str) -> Vec<u8> {
        lns_policy::artifact::to_config_blob(
            format!(
                "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\n\
                 metadata:\n  name: some-system\ncomponents:\n{components}"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    fn policy_blob() -> Vec<u8> {
        lns_policy::artifact::to_config_blob(b"network:\n  defaultVerdict: ask\n").unwrap()
    }

    async fn agent_client(command: &str, with_cred: bool) -> RefKeyedClient {
        let client = RefKeyedClient::default();
        put_artifact(
            &client,
            AGENT_REF,
            Family::Agent,
            &agent_blob(command, with_cred),
        )
        .await;
        client
    }

    fn full_agent_blob() -> Vec<u8> {
        lns_policy::artifact::to_config_blob(
            format!(
                "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
                 metadata:\n  name: some-agent\n\
                 spec:\n  image: {AGENT_IMAGE}\n  user: runner\n  \
                 resources:\n    cpus: 2\n    memoryMib: 3072\n  \
                 ports:\n    - {{ host: 9119, container: 9119 }}\n  \
                 volumes:\n    - {{ name: somedata, target: /opt/data }}\n"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    async fn full_agent_client() -> RefKeyedClient {
        let client = RefKeyedClient::default();
        put_artifact(&client, AGENT_REF, Family::Agent, &full_agent_blob()).await;
        client
    }

    async fn bundle_client(components: &str) -> RefKeyedClient {
        let client = RefKeyedClient::default();
        put_artifact(
            &client,
            BUNDLE_REF,
            Family::Bundle,
            &bundle_blob(components),
        )
        .await;
        put_artifact(&client, AGENT_REF, Family::Agent, &agent_blob("go", false)).await;
        put_artifact(&client, POLICY_REF, Family::Policy, &policy_blob()).await;
        client
    }

    fn cred(name: &str) -> CredentialRef {
        CredentialRef {
            name: name.into(),
            env: name.to_uppercase(),
        }
    }

    fn run_args(image: Option<&str>) -> RunArgs {
        RunArgs {
            image: image.map(str::to_string),
            cpus: None,
            mem: None,
            policy: None,
            sandbox_user: None,
            sandbox_uid: None,
            interactive: true,
            tty: true,
            detach: false,
            detach_keys: crate::cli::DetachChord(vec![0x10, 0x11]),
            workdir: None,
            env: Vec::new(),
            env_file: Vec::new(),
            publish: Vec::new(),
            volumes: Vec::new(),
            cmd: Vec::new(),
        }
    }

    #[tokio::test]
    async fn resolve_agent_ref_maps_image_command_and_credentials() {
        let client = agent_client("run it", true).await;
        let mut out = Vec::new();
        let resolved = resolve_agent_ref(AGENT_REF, &client, &mut out)
            .await
            .unwrap();
        assert_eq!(resolved.image, AGENT_IMAGE);
        assert_eq!(resolved.cmd, vec!["/bin/sh", "-c", "run it"]);
        assert_eq!(resolved.credentials[0].name, "some-provider");
        assert!(resolved.policy_path.is_none());
        assert!(String::from_utf8(out).unwrap().contains("resolved agent"));
    }

    #[tokio::test]
    async fn resolve_agent_ref_leaves_cmd_empty_when_the_agent_has_no_command() {
        let client = agent_client("", false).await;
        let resolved = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap();
        assert!(resolved.cmd.is_empty());
    }

    #[tokio::test]
    async fn resolve_agent_ref_rejects_an_image_where_an_agent_was_expected() {
        let client = RefKeyedClient::default();
        put_image(&client, AGENT_REF).await;
        let err = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not an agent artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_agent_ref_rejects_a_foreign_artifact_family() {
        let client = RefKeyedClient::default();
        put_artifact(&client, AGENT_REF, Family::Policy, &agent_blob("x", false)).await;
        let err = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not an agent artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_agent_ref_surfaces_a_missing_reference() {
        let client = RefKeyedClient::default();
        let err = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("no canned pull"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_agent_ref_surfaces_a_registry_error() {
        let client = RefKeyedClient {
            fail: Some("registry down".into()),
            ..Default::default()
        };
        let err = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("registry down"), "got: {err:#}");
    }

    #[tokio::test]
    async fn resolve_into_run_args_overlays_image_and_command_for_an_agent_ref() {
        let client = agent_client("run it", false).await;
        let (args, guard) =
            resolve_into_run_args(run_args(Some(AGENT_REF)), &client, &[], &mut Vec::new())
                .await
                .unwrap();
        assert_eq!(args.image.as_deref(), Some(AGENT_IMAGE));
        assert_eq!(args.cmd, vec!["/bin/sh", "-c", "run it"]);
        assert!(guard.is_none(), "a bare agent materializes no policy");
    }

    #[tokio::test]
    async fn resolve_into_run_args_warns_about_unprovisioned_credentials_but_proceeds() {
        let client = agent_client("go", true).await;
        let mut out = Vec::new();
        let (args, _) = resolve_into_run_args(run_args(Some(AGENT_REF)), &client, &[], &mut out)
            .await
            .unwrap();
        assert_eq!(args.image.as_deref(), Some(AGENT_IMAGE));
        let msg = String::from_utf8(out).unwrap();
        assert!(
            msg.contains("some-provider"),
            "expected a credential warning: {msg}"
        );
    }

    #[tokio::test]
    async fn resolve_agent_ref_maps_resources_user_ports_and_volumes() {
        let client = full_agent_client().await;
        let resolved = resolve_agent_ref(AGENT_REF, &client, &mut Vec::new())
            .await
            .unwrap();
        assert_eq!(resolved.cpus, Some(2));
        assert_eq!(resolved.mem, Some(3072));
        assert_eq!(resolved.sandbox_user.as_deref(), Some("runner"));
        assert_eq!(resolved.ports.len(), 1);
        assert_eq!(resolved.ports[0].host_port, 9119);
        assert_eq!(resolved.ports[0].container_port, 9119);
        assert!(resolved.ports[0].host_ip.is_loopback());
        assert_eq!(resolved.volumes.len(), 1);
        assert_eq!(resolved.volumes[0].name, "somedata");
        assert_eq!(resolved.volumes[0].target, "/opt/data");
    }

    #[tokio::test]
    async fn resolve_into_run_args_overlays_resources_user_ports_and_volumes() {
        let client = full_agent_client().await;
        let (args, _) =
            resolve_into_run_args(run_args(Some(AGENT_REF)), &client, &[], &mut Vec::new())
                .await
                .unwrap();
        assert_eq!(args.cpus, Some(2));
        assert_eq!(args.mem, Some(3072));
        assert_eq!(args.sandbox_user.as_deref(), Some("runner"));
        assert_eq!(args.publish.len(), 1);
        assert_eq!(args.publish[0].host_port, 9119);
        assert_eq!(args.volumes.len(), 1);
        assert_eq!(args.volumes[0].name, "somedata");
    }

    #[tokio::test]
    async fn resolve_into_run_args_keeps_explicit_resources_ports_and_volumes_over_the_agents() {
        let client = full_agent_client().await;
        let mut args = run_args(Some(AGENT_REF));
        args.cpus = Some(8);
        args.mem = Some(1024);
        args.sandbox_user = Some("override".into());
        args.publish = vec![lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            host_port: 1,
            container_port: 1,
            protocol: lns_ipc::Protocol::Tcp,
        }];
        args.volumes = vec![lns_ipc::VolumeMount {
            name: "mine".into(),
            target: "/mnt".into(),
            read_only: false,
        }];
        let (args, _) = resolve_into_run_args(args, &client, &[], &mut Vec::new())
            .await
            .unwrap();
        assert_eq!(args.cpus, Some(8), "explicit --cpus must win");
        assert_eq!(args.mem, Some(1024), "explicit --mem must win");
        assert_eq!(args.sandbox_user.as_deref(), Some("override"));
        assert_eq!(args.publish[0].host_port, 1, "explicit -p must win");
        assert_eq!(args.volumes[0].name, "mine", "explicit -v must win");
    }

    #[tokio::test]
    async fn resolve_into_run_args_keeps_an_explicit_command_over_the_agents() {
        let client = agent_client("agent cmd", false).await;
        let mut args = run_args(Some(AGENT_REF));
        args.cmd = vec!["echo".into(), "hi".into()];
        let (args, _) = resolve_into_run_args(args, &client, &[], &mut Vec::new())
            .await
            .unwrap();
        assert_eq!(args.cmd, vec!["echo", "hi"], "the --cmd override must win");
    }

    #[tokio::test]
    async fn resolve_into_run_args_passes_a_plain_image_through_untouched() {
        let client = RefKeyedClient::default();
        let (args, guard) = resolve_into_run_args(
            run_args(Some("docker.io/library/alpine:3.20")),
            &client,
            &[],
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(args.image.as_deref(), Some("docker.io/library/alpine:3.20"));
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn resolve_into_run_args_passes_an_imageless_run_through_untouched() {
        let client = RefKeyedClient::default();
        let (args, _) = resolve_into_run_args(run_args(None), &client, &[], &mut Vec::new())
            .await
            .unwrap();
        assert!(args.image.is_none());
    }

    #[tokio::test]
    async fn resolve_into_run_args_rejects_a_non_runnable_family() {
        let client = RefKeyedClient::default();
        let result = resolve_into_run_args(
            run_args(Some("localhost:5000/org/acme/policies/pii:v1")),
            &client,
            &[],
            &mut Vec::new(),
        )
        .await;
        let err = result.err().expect("a policy ref is not runnable");
        assert!(
            format!("{err:#}").contains("cannot be run directly"),
            "got: {err:#}"
        );
    }

    #[test]
    fn qualify_component_ref_prepends_the_bundle_host_to_a_host_less_component() {
        assert_eq!(
            qualify_component_ref(BUNDLE_REF, "org/acme/agents/some-agent:v1"),
            AGENT_REF
        );
    }

    #[test]
    fn qualify_component_ref_leaves_an_already_qualified_component_untouched() {
        assert_eq!(
            qualify_component_ref(BUNDLE_REF, "other.example:7000/x/agents/a:v1"),
            "other.example:7000/x/agents/a:v1"
        );
    }

    #[test]
    fn qualify_component_ref_passes_through_when_the_bundle_has_no_host() {
        assert_eq!(
            qualify_component_ref("org/acme/bundles/x:v1", "org/acme/agents/a:v1"),
            "org/acme/agents/a:v1"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_resolves_the_agent_and_materializes_the_policy() {
        let client = bundle_client(
            "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
             policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n",
        )
        .await;
        let path;
        {
            let resolved = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
                .await
                .unwrap();
            assert_eq!(resolved.image, AGENT_IMAGE);
            assert_eq!(resolved.cmd, vec!["/bin/sh", "-c", "go"]);
            let p = resolved.policy_path.clone().expect("policy materialized");
            assert!(p.exists(), "policy temp file exists during the run");
            assert_eq!(std::fs::read(&p).unwrap(), policy_blob());
            path = p;
        }
        assert!(
            !path.exists(),
            "the policy temp file is removed when the run ends"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_without_a_policy_component_materializes_no_policy() {
        let client =
            bundle_client("  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n").await;
        let resolved = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap();
        assert!(resolved.policy_path.is_none());
    }

    #[tokio::test]
    async fn resolve_bundle_ref_notes_unapplied_components() {
        let client = bundle_client(
            "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
             tools:\n    - { ref: org/acme/tools/some-tool:v1 }\n  \
             sandbox:\n    ref: org/acme/sandboxes/some-runtime:v1\n  \
             knowledge:\n    - { ref: org/acme/knowledge/some-runbook:v1 }\n",
        )
        .await;
        let mut out = Vec::new();
        resolve_bundle_ref(BUNDLE_REF, &client, &mut out)
            .await
            .unwrap();
        let msg = String::from_utf8(out).unwrap();
        assert!(msg.contains("tools"), "got: {msg}");
        assert!(msg.contains("sandbox"), "got: {msg}");
        assert!(msg.contains("knowledge"), "got: {msg}");
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_a_bundle_with_no_agent() {
        let client =
            bundle_client("  policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n").await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("no agent component"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_a_multi_agent_bundle() {
        let client = bundle_client(
            "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n    \
             - { ref: org/acme/agents/other:v1 }\n",
        )
        .await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("multi-agent"), "got: {err:#}");
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_a_policy_component_that_is_an_image() {
        let client = RefKeyedClient::default();
        put_artifact(
            &client,
            BUNDLE_REF,
            Family::Bundle,
            &bundle_blob(
                "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
                 policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n",
            ),
        )
        .await;
        put_artifact(&client, AGENT_REF, Family::Agent, &agent_blob("go", false)).await;
        put_image(&client, POLICY_REF).await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a policy artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_a_policy_component_of_a_foreign_family() {
        let client = RefKeyedClient::default();
        put_artifact(
            &client,
            BUNDLE_REF,
            Family::Bundle,
            &bundle_blob(
                "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
                 policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n",
            ),
        )
        .await;
        put_artifact(&client, AGENT_REF, Family::Agent, &agent_blob("go", false)).await;
        put_artifact(&client, POLICY_REF, Family::Tool, &policy_blob()).await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a policy artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_a_foreign_artifact_family() {
        let client = RefKeyedClient::default();
        put_artifact(&client, BUNDLE_REF, Family::Agent, &agent_blob("x", false)).await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a bundle artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_bundle_ref_rejects_an_image_where_a_bundle_was_expected() {
        let client = RefKeyedClient::default();
        put_image(&client, BUNDLE_REF).await;
        let err = resolve_bundle_ref(BUNDLE_REF, &client, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a bundle artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn resolve_into_run_args_materializes_the_bundle_policy_as_the_run_policy() {
        let client = bundle_client(
            "  agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
             policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n",
        )
        .await;
        let (args, guard) =
            resolve_into_run_args(run_args(Some(BUNDLE_REF)), &client, &[], &mut Vec::new())
                .await
                .unwrap();
        let guard = guard.expect("bundle policy guard kept alive");
        assert_eq!(args.policy.as_deref(), Some(guard.path()));
    }

    #[test]
    fn available_credentials_lists_usable_entries_and_omits_denied_ones() {
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), CredentialEntry::HostDetect);
        state.insert(
            "some-stored".into(),
            CredentialEntry::Stored { value: "x".into() },
        );
        state.insert("some-denied".into(), CredentialEntry::Deny);
        let mut available = available_credentials(&state);
        available.sort();
        assert_eq!(available, vec!["some-provider", "some-stored"]);
    }

    #[test]
    fn missing_credentials_is_empty_when_every_required_credential_is_available() {
        let required = vec![cred("some-provider")];
        assert!(missing_credentials(&required, &["some-provider".to_string()]).is_empty());
    }

    #[test]
    fn missing_credentials_lists_only_the_unprovisioned_ones() {
        let required = vec![cred("some-provider"), cred("some-oauth")];
        let missing = missing_credentials(&required, &["some-provider".to_string()]);
        assert_eq!(missing, vec!["some-oauth".to_string()]);
    }

    #[test]
    fn missing_credentials_is_empty_when_nothing_is_required() {
        assert!(missing_credentials(&[], &[]).is_empty());
    }

    #[test]
    fn missing_credentials_warning_names_each_credential_and_its_connect_hint() {
        let msg = missing_credentials_warning(&["some-oauth".to_string()]);
        assert!(msg.contains("some-oauth"), "got: {msg}");
        assert!(msg.contains("`lns connect some-oauth`"), "got: {msg}");
    }
}
