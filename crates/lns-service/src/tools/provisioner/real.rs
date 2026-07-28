use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::{DRIVER, EngineArtifacts};
use crate::download::{Fetcher, RealFetcher, RealFs};
use crate::tools::{ProvisionError, ProvisionTarget, StagedTool, ToolProvisioner, ToolRef, mise};
use crate::{cache, composefs, guest_tools, image, ingest, initramfs, kernel, upperfs, vm};

const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DRIVER_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const BOOT_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) struct MiseProvisioner {
    pub scratch_id: String,
}

impl ToolProvisioner for MiseProvisioner {
    fn newest_version(&self, name: &str) -> impl Future<Output = Result<String>> + Send {
        let name = name.to_string();
        async move {
            let url = mise::version_index_url(&name);
            let body = RealFetcher {
                max_bytes: MAX_INDEX_BYTES,
            }
            .fetch(&url)
            .await
            .with_context(|| format!("querying the tool version index at {url}"))?;
            let body = String::from_utf8(body)
                .with_context(|| format!("the version index at {url} is not text"))?;
            lns_artifact::tools::resolve_from_index(&name, lns_artifact::tools::LATEST, &body)
        }
    }

    fn provision(
        &self,
        requests: &[ToolRef],
        target: &ProvisionTarget,
    ) -> impl Future<Output = Result<Vec<StagedTool>, ProvisionError>> + Send {
        let requests = requests.to_vec();
        let target = *target;
        let scratch_id = self.scratch_id.clone();
        async move {
            run_provisioner(&scratch_id, &requests, &target)
                .await
                .map_err(|e| match e.downcast::<ProvisionError>() {
                    Ok(provision) => provision,
                    Err(other) => ProvisionError::Engine(format!("{other:#}")),
                })
        }
    }
}

/// The guest ended before it was reachable: surface why rather than collapsing every cause into "never became reachable".
fn boot_failure(outcome: Result<Result<()>, tokio::task::JoinError>) -> anyhow::Error {
    match outcome {
        Ok(Ok(())) => anyhow::anyhow!("the provisioner guest exited before it was reachable"),
        Ok(Err(e)) => e.context("booting the provisioner guest"),
        Err(e) => anyhow::anyhow!("the provisioner guest boot task panicked: {e}"),
    }
}

async fn run_provisioner(
    scratch_id: &str,
    requests: &[ToolRef],
    target: &ProvisionTarget,
) -> Result<Vec<StagedTool>> {
    let cache_dir = cache::root()?;
    let staging = cache_dir.join("runs").join(scratch_id).join("tool-staging");
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;

    crate::log::progress("Provisioning", "declared tools", 0, 0);
    let artifacts = ensure_engine_artifacts(&cache_dir, target).await?;
    let driver = super::render_driver(requests);
    let provisioner_specs = super::provisioner_runtime_specs(&artifacts, driver);

    let layer_cache = crate::oci_layer_cache::LayerCache::new(cache_dir.join("layers"));
    let content_store = crate::content_store::ContentStore::new(cache_dir.join("content"));
    let descriptor_builder = composefs::descriptor::DescriptorBuilder::new(cache_dir.clone());

    let rootfs_ref = mise::manifest()
        .rootfs_reference(target.libc, target.arch)?
        .to_string();
    let cmd: [String; 0] = [];
    let want_arch = image::want_arch();
    let (tools_res, image_res, kernel_res, upper_res) = tokio::join!(
        async {
            let tools = guest_tools::ensure().await?;
            let initrd = initramfs::build(&tools).await?;
            Ok::<_, anyhow::Error>((tools, initrd))
        },
        ingest::run(
            Some(&rootfs_ref),
            &cmd,
            &want_arch,
            &layer_cache,
            image::pull,
        ),
        kernel::ensure(),
        upperfs::provision(scratch_id),
    );
    let (guest_tools, initrd) = tools_res?;
    let mut rootfs = image_res.context("pulling the provisioner rootfs")?;
    let kernel_path = kernel_res?;
    let upper_disk = upper_res?;

    let runtime_layer = crate::runtime_layer::for_run(
        false,
        &content_store,
        &guest_tools,
        None,
        &provisioner_specs,
    )?;
    let layers = std::mem::take(&mut rootfs.bytes);
    let layer_digests = std::mem::take(&mut rootfs.digests);
    let probe = composefs::descriptor::DescriptorRequest {
        layer_digests: &layer_digests,
        layers: &layers,
        runtime_layer: runtime_layer.as_ref(),
    };
    let descriptor = match descriptor_builder.cached(&probe)? {
        Some(hit) => hit,
        None => {
            let descriptor_cs = content_store.clone();
            tokio::task::spawn_blocking(move || {
                descriptor_builder.build(
                    &descriptor_cs,
                    &composefs::descriptor::DescriptorRequest {
                        layer_digests: &layer_digests,
                        layers: &layers,
                        runtime_layer: runtime_layer.as_ref(),
                    },
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("provisioner descriptor build panicked: {e}"))??
        }
    };

    let run_as = vm::resolve_run_as(Some("0"), Some(0), None);
    let argv = vec!["/bin/sh".to_string(), DRIVER.to_string()];
    let exec = vm::ExecSpec::for_run(&run_as, None, &argv, rootfs.config.as_ref(), None);

    #[cfg(target_os = "macos")]
    let console_fd = vm::diag_console::spawn(
        upper_disk
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| Path::new(".").to_path_buf())
            .join("console.log"),
        false,
    )?;

    let (connector_tx, connector_rx) =
        tokio::sync::oneshot::channel::<Arc<dyn vm::GuestTransport>>();
    let spec = vm::VmSpec {
        run_id: scratch_id.to_string(),
        cpus: 2,
        memory_mib: 2048,
        kernel: kernel_path,
        initrd,
        composefs_descriptor: descriptor.path.clone(),
        content_share: content_store.root().to_path_buf(),
        content_tag: "lns-content".to_string(),
        descriptor_sha256: Some(descriptor.descriptor_sha256.clone()),
        upper_disk,
        volumes: Vec::new(),
        binds: vec![vm::BindAttachment {
            host_source: staging.clone(),
            target: super::STAGING.to_string(),
            read_only: false,
            dropped_paths: Vec::new(),
        }],
        workload_uid: run_as.uid,
        workload_gid: vm::host_known_workload_gid(&run_as),
        vsock: None,
        connector_tx: Some(connector_tx),
        #[cfg(target_os = "macos")]
        console_fd,
        debug: false,
        exec,
    };

    let mut vm_task = tokio::spawn(vm::boot(spec, None));
    let connector = tokio::select! {
        biased;
        r = &mut vm_task => {
            return Err(boot_failure(r));
        }
        c = connector_rx => c.context("the provisioner guest never became reachable")?,
        // One wedged VMM would otherwise hold the provision lock for the service's lifetime.
        _ = tokio::time::sleep(BOOT_BUDGET) => {
            vm_task.abort();
            anyhow::bail!("the provisioner guest did not become reachable within {BOOT_BUDGET:?}");
        }
    };
    let _stop_guard = vm::VmStopGuard::new(connector.clone());

    let env: Vec<String> = mise::provision_env()
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let timeout =
        super::driver_timeout_secs(std::env::var("LNS_TOOLS_TIMEOUT_SECS").ok().as_deref());
    let (captured, exit_code) = vm::session_client::capture_session_exec(
        connector.as_ref(),
        argv,
        env,
        std::time::Duration::from_secs(timeout),
        MAX_DRIVER_OUTPUT_BYTES,
    )
    .await
    .context("driving the provisioner install script")?;

    let results =
        super::parse_driver_output(&captured.stdout, &captured.stderr, exit_code, requests)?;
    Ok(super::staged_tools_from_results(
        requests, &results, &staging,
    )?)
}

pub(crate) async fn ensure_engine_artifacts(
    cache_dir: &Path,
    target: &ProvisionTarget,
) -> Result<EngineArtifacts> {
    super::ensure_engine_artifacts_with(
        &RealFetcher {
            max_bytes: MAX_ARTIFACT_BYTES,
        },
        &RealFs,
        mise::manifest(),
        cache_dir,
        target,
    )
    .await
}

/// The musl companion trees a workload guest needs beside node/bun tool trees, injected at their canonical library paths.
pub(crate) async fn workload_companion_specs(
    cache_dir: &Path,
    target: &ProvisionTarget,
) -> Result<Vec<crate::runtime_layer::RuntimeFileSpec>> {
    let artifacts = ensure_engine_artifacts(cache_dir, target).await?;
    Ok(artifacts.workload_companion_specs)
}
