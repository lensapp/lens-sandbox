//! The canned service both namespaces' unit suites drive: one scripted reply per request kind, and the fixtures they build those replies from.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use lns_ipc::{Request, Response};
use tokio::io::AsyncWriteExt;

use crate::service::client::{BoxFuture, SandboxService};

pub(crate) struct CannedService {
    response: Response,
    stats_response: Option<Response>,
    inspect_image_response: Option<Response>,
    remove_image_response: Option<Response>,
    list_prunable_response: Option<Response>,
    list_images_response: Option<Response>,
    frames: Vec<Vec<u8>>,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl CannedService {
    pub fn new(response: Response) -> Self {
        Self {
            response,
            stats_response: None,
            inspect_image_response: None,
            remove_image_response: None,
            list_prunable_response: None,
            list_images_response: None,
            frames: Vec::new(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_list_images(response: Response, list_images: Response) -> Self {
        Self {
            list_images_response: Some(list_images),
            ..Self::new(response)
        }
    }

    pub fn with_list_prunable(response: Response, list_prunable: Response) -> Self {
        Self {
            list_prunable_response: Some(list_prunable),
            ..Self::new(response)
        }
    }

    pub fn with_stats(response: Response, stats_response: Response) -> Self {
        Self {
            stats_response: Some(stats_response),
            ..Self::new(response)
        }
    }

    pub fn with_inspect_image(run_response: Response, image_response: Response) -> Self {
        Self {
            inspect_image_response: Some(image_response),
            ..Self::new(run_response)
        }
    }

    pub fn with_remove_image(run_response: Response, remove_response: Response) -> Self {
        Self {
            remove_image_response: Some(remove_response),
            ..Self::new(run_response)
        }
    }

    pub fn with_frames(frames: Vec<Vec<u8>>) -> Self {
        Self {
            frames,
            ..Self::new(Response::Pong)
        }
    }
}

pub(crate) fn sandbox_inspection(tools: Vec<String>) -> Response {
    sandbox_inspection_with_digest(tools, format!("sha256:{}", "a".repeat(64)))
}

pub(crate) fn sandbox_inspection_with_digest(tools: Vec<String>, digest: String) -> Response {
    Response::ImageInspected {
        inspection: lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference: "ghcr.io/team/hermes:1.4.0".into(),
            digest,
            image: "docker.io/library/alpine@sha256:abc".into(),
            workdir: None,
            user: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools,
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
        })),
    }
}

pub(crate) fn cached_info(reference: &str) -> lns_ipc::ImageInfo {
    lns_ipc::ImageInfo {
        reference: reference.to_string(),
        kind: lns_ipc::CachedKind::Sandbox,
        digest: format!("sha256:{}", "a".repeat(64)),
        size_bytes: 0,
        layers: 0,
        pulled: "2026-01-01T00:00:00Z".into(),
        in_use_by: None,
    }
}

pub(crate) fn pulled_response() -> Response {
    Response::ImagePulled {
        image: lns_ipc::ImageInfo {
            kind: lns_ipc::CachedKind::Sandbox,
            reference: "ghcr.io/team/hermes:1.4.0".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 1024,
            layers: 1,
            pulled: "2026-01-01T00:00:00Z".into(),
            in_use_by: None,
        },
        warnings: Vec::new(),
    }
}

impl SandboxService for CannedService {
    type Stream = tokio::io::DuplexStream;
    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
        let response = match &request {
            Request::RunStats { .. } => self
                .stats_response
                .clone()
                .unwrap_or_else(|| self.response.clone()),
            Request::InspectImage { .. } => self
                .inspect_image_response
                .clone()
                .unwrap_or_else(|| self.response.clone()),
            Request::RemoveImage { .. } => self
                .remove_image_response
                .clone()
                .unwrap_or_else(|| self.response.clone()),
            Request::ListPrunableImages => self
                .list_prunable_response
                .clone()
                .unwrap_or(Response::ImageList { images: Vec::new() }),
            Request::ListImages => self
                .list_images_response
                .clone()
                .unwrap_or_else(|| self.response.clone()),
            _ => self.response.clone(),
        };
        self.requests.lock().unwrap().push(request);
        Box::pin(async move { Ok(response) })
    }
    fn open_stream(&self, _request: Request) -> BoxFuture<'_, Result<Self::Stream>> {
        let frames = self.frames.clone();
        Box::pin(async move {
            if frames.is_empty() {
                bail!("the daemon refused the stream");
            }
            Ok(stream_with(&frames).await)
        })
    }
    fn aux_socket(&self) -> Option<PathBuf> {
        None
    }
    fn load_policy(&self, _path: &str) -> Option<serde_json::Value> {
        None
    }
}

pub(crate) async fn stream_with(frames: &[Vec<u8>]) -> tokio::io::DuplexStream {
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let payload: Vec<u8> = frames.concat();
    tokio::spawn(async move {
        let _ = server.write_all(&payload).await;
    });
    client
}
