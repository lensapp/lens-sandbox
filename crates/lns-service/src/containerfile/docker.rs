//! The host Docker daemon as a build engine: slice 7 of lensapp/lens-sandbox#393.
//!
//! A machine whose `build.engine` says `docker` sends the Containerfile and its context to the
//! daemon's build endpoint over the Docker Engine API, exports the image the daemon answers with,
//! and imports it into the same layer store the build guest's images land in. The document's
//! `egress` and `credentials` decide nothing inside the daemon, which is why every line about such
//! an image says so.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::cache::Kind;
use super::context::{ContextFs, EntryKind};
use super::executor::{self, Base, BuildPlan, Built, Cached};
use super::image::BuiltImage;
use super::tar_layer::LayerBlob;

/// The Docker Engine API version every request here is pinned to: 1.43 is Docker 24's, old enough for every daemon in use and new enough for `platform` on a build.
pub(crate) const API_VERSION: &str = "v1.43";

/// The repository a daemon build is tagged under before it is exported: a host no registry resolves, so the tag can only ever be read back on this machine.
pub(crate) const BUILD_TAG_REPOSITORY: &str = "lns-build.local/docker";

/// How to put this machine's build back in the guest, named wherever a daemon build is refused or disclosed.
pub(crate) const SWITCH_OFF_RECIPE: &str =
    "run `lns config set build.engine lns` to build in a guest instead";

/// One HTTP exchange with the daemon: the caller writes the whole request and reads until the daemon closes, so no framing decision lives in the transport.
pub(crate) trait Daemon {
    /// The socket this daemon was reached at, which every refusal names.
    fn socket(&self) -> &str;
    async fn round_trip(&self, request: &[u8]) -> Result<Vec<u8>>;
}

/// One request to the Engine API, as the daemon reads it off the socket.
pub(crate) struct ApiRequest {
    pub method: &'static str,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub content_type: Option<&'static str>,
    pub body: Vec<u8>,
}

impl ApiRequest {
    fn get(path: &str) -> Self {
        Self {
            method: "GET",
            path: format!("/{API_VERSION}{path}"),
            query: Vec::new(),
            content_type: None,
            body: Vec::new(),
        }
    }

    fn post(path: &str, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            method: "POST",
            path: format!("/{API_VERSION}{path}"),
            query: Vec::new(),
            content_type: Some(content_type),
            body,
        }
    }

    fn delete(path: &str) -> Self {
        Self {
            method: "DELETE",
            path: format!("/{API_VERSION}{path}"),
            query: Vec::new(),
            content_type: None,
            body: Vec::new(),
        }
    }

    fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.query.push((key.to_string(), value.into()));
        self
    }
}

/// What the daemon answered, once the framing is off it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The bytes one request is on the wire: `Connection: close` so the whole answer is what the transport reads to EOF.
pub(crate) fn encode(request: &ApiRequest) -> Vec<u8> {
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(request.query.iter())
        .finish();
    let target = match query.is_empty() {
        true => request.path.clone(),
        false => format!("{}?{query}", request.path),
    };
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: docker\r\nAccept: */*\r\nConnection: close\r\n",
        request.method
    );
    if let Some(content_type) = request.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&request.body);
    bytes
}

/// The status and the body of a whole HTTP/1.1 answer, chunked or not; a daemon that answered nothing at all is not an answer.
pub(crate) fn decode(bytes: &[u8]) -> Result<ApiResponse> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("the Docker daemon answered no complete HTTP header")?;
    let head = String::from_utf8_lossy(&bytes[..split]).to_string();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .with_context(|| format!("the Docker daemon answered no HTTP status line: {head:?}"))?;
    let chunked = lines.any(|line| {
        let (name, value) = line.split_once(':').unwrap_or((line, ""));
        name.eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
    });
    let rest = &bytes[split + 4..];
    let body = match chunked {
        true => dechunk(rest)?,
        false => rest.to_vec(),
    };
    Ok(ApiResponse { status, body })
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .context("a chunked answer from the Docker daemon ended mid-chunk")?;
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&rest[..end])
                .split(';')
                .next()
                .unwrap_or("")
                .trim(),
            16,
        )
        .context("a chunked answer from the Docker daemon named no chunk size")?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if rest.len() < size {
            bail!("a chunked answer from the Docker daemon ended mid-chunk");
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size.min(rest.len())..];
        rest = rest.strip_prefix(b"\r\n").unwrap_or(rest);
    }
}

async fn call<D: Daemon>(daemon: &D, request: ApiRequest) -> Result<ApiResponse> {
    let path = request.path.clone();
    let answer = daemon
        .round_trip(&encode(&request))
        .await
        .with_context(|| refuse_an_unreachable_daemon(daemon.socket()))?;
    decode(&answer).with_context(|| format!("reading what the Docker daemon answered to {path}"))
}

/// A machine whose switch says `docker` and whose daemon does not answer is refused by name rather than built in a guest behind the author's back (§3.1.1).
pub(crate) fn refuse_an_unreachable_daemon(socket: &str) -> String {
    format!(
        "this machine's build.engine is docker, and no Docker daemon answered at {socket}; \
         start the daemon, point build.dockerSocket at the socket it listens on, or {SWITCH_OFF_RECIPE}"
    )
}

/// Whether a daemon answers at all, asked before anything is sent to it, so a build never fails halfway through a context upload.
pub(crate) async fn ping<D: Daemon>(daemon: &D) -> Result<()> {
    let answer = call(daemon, ApiRequest::get("/_ping")).await?;
    match answer.status {
        200 => Ok(()),
        status => bail!(
            "the Docker daemon at {} answered {status} to a ping; {SWITCH_OFF_RECIPE}",
            daemon.socket()
        ),
    }
}

/// What one daemon build is asked for: the context, the file inside it, the `ARG` defaults it is built with, and the platform it is built for.
pub(crate) struct DockerBuild {
    pub context_tar: Vec<u8>,
    /// The Containerfile's path inside the context tar, which is what `dockerfile` names.
    pub containerfile: String,
    pub build_args: Vec<(String, String)>,
    pub platform: String,
    pub tag: String,
    pub rebuild: bool,
}

/// The tag a daemon build is asked to write, derived from the key so two builds of one document never collide and a rebuild replaces its own.
pub(crate) fn build_tag(key: &str) -> String {
    format!(
        "{BUILD_TAG_REPOSITORY}:{}",
        key.strip_prefix("sha256:").unwrap_or(key)
    )
}

/// `POST /build`: the context goes up as a tar, and the daemon answers a stream of JSON lines whose last word on failure is the build's own.
pub(crate) async fn build<D: Daemon>(daemon: &D, request: &DockerBuild) -> Result<String> {
    let args = serde_json::to_string(
        &request
            .build_args
            .iter()
            .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
            .collect::<serde_json::Map<String, serde_json::Value>>(),
    )
    .context("serializing the build arguments the Containerfile declares")?;
    let call_request = ApiRequest::post("/build", "application/x-tar", request.context_tar.clone())
        .with("dockerfile", request.containerfile.clone())
        .with("t", request.tag.clone())
        .with("platform", request.platform.clone())
        .with("buildargs", args)
        .with("rm", "1")
        .with(
            "nocache",
            match request.rebuild {
                true => "1",
                false => "0",
            },
        );
    let answer = call(daemon, call_request).await?;
    if answer.status != 200 {
        bail!(
            "the Docker daemon refused the build with {}: {}",
            answer.status,
            String::from_utf8_lossy(&answer.body).trim()
        );
    }
    refuse_a_failed_build(&answer.body)?;
    Ok(request.tag.clone())
}

/// The daemon answers a build with 200 before it runs it, so a failure is a line of the stream and not a status.
fn refuse_a_failed_build(body: &[u8]) -> Result<()> {
    for line in String::from_utf8_lossy(body).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(error) = value["error"].as_str() {
            bail!("the Docker daemon failed the build: {}", error.trim());
        }
    }
    Ok(())
}

/// `GET /images/{name}/get`: the built image as a tar, which is what `docker save` writes.
pub(crate) async fn export<D: Daemon>(daemon: &D, tag: &str) -> Result<Vec<u8>> {
    let answer = call(daemon, ApiRequest::get(&format!("/images/{tag}/get"))).await?;
    if answer.status != 200 {
        bail!(
            "the Docker daemon would not export {tag}, answering {}: {}",
            answer.status,
            String::from_utf8_lossy(&answer.body).trim()
        );
    }
    Ok(answer.body)
}

/// The build context as the daemon reads it: every regular file and directory, and, as §3.1.1 says of a context, no symlink.
pub(crate) fn context_tar<F: ContextFs>(
    fs: &F,
    context: &Path,
    pinned: &PinnedContainerfile<'_>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut bytes);
        pack(fs, context, "", &mut builder, pinned)?;
        builder.finish().context("closing the build context tar")?;
    }
    Ok(bytes)
}

/// The Containerfile as the daemon reads it out of the context: named where it sits inside the tar, and written with the `FROM` the key stands on.
pub(crate) struct PinnedContainerfile<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

fn pack<F: ContextFs, W: std::io::Write>(
    fs: &F,
    directory: &Path,
    prefix: &str,
    builder: &mut tar::Builder<W>,
    pinned: &PinnedContainerfile<'_>,
) -> Result<()> {
    let mut names = fs
        .entries(directory)
        .with_context(|| format!("reading the build context at {}", directory.display()))?;
    names.sort();
    for name in names {
        let path = directory.join(&name);
        let relative = match prefix.is_empty() {
            true => name.clone(),
            false => format!("{prefix}/{name}"),
        };
        let Some(meta) = fs
            .meta(&path)
            .with_context(|| format!("reading {} in the build context", path.display()))?
        else {
            continue;
        };
        match meta.kind {
            EntryKind::Directory => {
                let mut header = entry_header(meta.mode, 0, tar::EntryType::Directory);
                builder
                    .append_data(&mut header, format!("{relative}/"), std::io::empty())
                    .with_context(|| format!("writing {relative} into the build context tar"))?;
                pack(fs, &path, &relative, builder, pinned)?;
            }
            EntryKind::Regular => {
                let content = match relative == pinned.path {
                    true => pinned.text.as_bytes().to_vec(),
                    false => fs.read(&path).with_context(|| {
                        format!("reading {} in the build context", path.display())
                    })?,
                };
                let mut header =
                    entry_header(meta.mode, content.len() as u64, tar::EntryType::Regular);
                builder
                    .append_data(&mut header, &relative, content.as_slice())
                    .with_context(|| format!("writing {relative} into the build context tar"))?;
            }
            EntryKind::Symlink => continue,
        }
    }
    Ok(())
}

fn entry_header(mode: u32, size: u64, kind: tar::EntryType) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_mode(mode);
    header.set_size(size);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_entry_type(kind);
    header
}

/// What a `docker save` tar holds, read back: the image config as the daemon wrote it, and one blob per layer in the order the config stacks them.
#[derive(Debug)]
pub(crate) struct SavedImage {
    pub config: String,
    pub layers: Vec<LayerBlob>,
}

/// Read an exported image out of the tar the daemon answered with.
pub(crate) fn read_saved_image(tar_bytes: &[u8]) -> Result<SavedImage> {
    let entries = tar_entries(tar_bytes)?;
    let manifest: serde_json::Value = serde_json::from_slice(
        entries
            .iter()
            .find(|(name, _)| name == "manifest.json")
            .map(|(_, bytes)| bytes.as_slice())
            .context("the exported image holds no manifest.json, so it is no image lns can read")?,
    )
    .context("reading the exported image's manifest.json")?;
    let first = manifest
        .get(0)
        .context("the exported image's manifest.json names no image")?;
    let config_name = first["Config"]
        .as_str()
        .context("the exported image names no config blob")?;
    let config = String::from_utf8(named(&entries, config_name)?)
        .context("the exported image's config is not text")?;
    let mut layers = Vec::new();
    for name in first["Layers"]
        .as_array()
        .context("the exported image names no layers")?
    {
        let name = name
            .as_str()
            .context("the exported image names a layer that is no path")?;
        let bytes = named(&entries, name)?;
        layers.push(LayerBlob {
            digest: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
            entries: 0,
            bytes,
        });
    }
    Ok(SavedImage { config, layers })
}

fn named(entries: &[(String, Vec<u8>)], name: &str) -> Result<Vec<u8>> {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, bytes)| bytes.clone())
        .with_context(|| format!("the exported image names {name}, which it does not carry"))
}

fn tar_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = Vec::new();
    for entry in archive
        .entries()
        .context("reading the image the Docker daemon exported")?
    {
        let mut entry = entry.context("reading the image the Docker daemon exported")?;
        let name = entry
            .path()
            .context("reading a path out of the exported image")?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        let mut content = Vec::new();
        std::io::copy(&mut entry, &mut content)
            .with_context(|| format!("reading {name} out of the exported image"))?;
        entries.push((name, content));
    }
    Ok(entries)
}

/// The OCI image an exported one becomes: the daemon's own config, and its layers described the way the layer store already holds every other built image's.
pub(crate) fn as_oci_image(saved: &SavedImage) -> Result<BuiltImage> {
    if saved.layers.is_empty() {
        bail!("the Docker daemon exported an image with no layer, which nothing can boot");
    }
    let manifest = oci_client::manifest::OciImageManifest {
        schema_version: 2,
        media_type: Some(oci_client::manifest::OCI_IMAGE_MEDIA_TYPE.to_string()),
        config: oci_client::manifest::OciDescriptor {
            media_type: oci_client::manifest::IMAGE_CONFIG_MEDIA_TYPE.to_string(),
            digest: format!(
                "sha256:{}",
                hex::encode(Sha256::digest(saved.config.as_bytes()))
            ),
            size: saved.config.len() as i64,
            ..Default::default()
        },
        layers: saved
            .layers
            .iter()
            .map(|layer| oci_client::manifest::OciDescriptor {
                media_type: layer_media_type(&layer.bytes).to_string(),
                digest: layer.digest.clone(),
                size: layer.size() as i64,
                ..Default::default()
            })
            .collect(),
        annotations: None,
        artifact_type: None,
        subject: None,
    };
    let bytes =
        serde_json::to_vec(&manifest).context("serializing the imported image's manifest")?;
    Ok(BuiltImage {
        manifest_digest: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
        manifest,
        config: saved.config.clone(),
    })
}

/// A daemon may export a layer either way, and the descriptor has to say which — the ingest reads both, but a consumer of the published image reads the media type.
fn layer_media_type(bytes: &[u8]) -> &'static str {
    match bytes.starts_with(&[0x1f, 0x8b]) {
        true => oci_client::manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE,
        false => super::tar_layer::LAYER_MEDIA_TYPE,
    }
}

/// What a daemon build needs of this machine beyond the daemon: the base its key stands on, what this machine already built for that key, and where the image the daemon exported lands.
pub(crate) trait DockerHost {
    /// The digest-pinned reference the `FROM` resolves to, read off the registry with no layer fetched — the daemon pulls its own base.
    async fn peek_base(&self, image: &str) -> Result<Base>;
    async fn cached(&self, kind: Kind, key: &str) -> Option<Cached>;
    async fn remember(&self, kind: Kind, key: &str, built: &Cached);
    /// Take an image another engine built into the local store, with every layer it carries, and answer with the reference the store holds it under.
    async fn adopt(&self, built: &BuiltImage, layers: &[LayerBlob]) -> Result<String>;
}

/// Where a daemon build reads its context from: the directory beside the document, and the file inside it the daemon is told to build.
pub(crate) struct DaemonBuild<'a> {
    pub context: &'a Path,
    /// The Containerfile's path inside the context, as `dockerfile` names it.
    pub containerfile: String,
}

/// Build a Containerfile on the host Docker daemon and import what it answers with, so the key, the publish and the index are the ones the build guest's images get (§3.1.1).
pub(crate) async fn build_image<H: DockerHost, D: Daemon, F: ContextFs>(
    host: &H,
    daemon: &D,
    fs: &F,
    plan: &BuildPlan<'_>,
    where_from: &DaemonBuild<'_>,
) -> Result<Built> {
    let preamble = executor::preamble(plan.file)?;
    let base = host
        .peek_base(&preamble.image)
        .await
        .with_context(|| format!("line {}: FROM {}", preamble.line, preamble.image))?;
    let key = super::key::image_key(&base.reference, plan.text, plan.context_hash, plan.arch);
    if !plan.rebuild
        && let Some(cached) = host.cached(Kind::Image, &key).await
    {
        return Ok(Built {
            reference: cached.reference,
            layers: 0,
            key,
            reused: true,
            reused_steps: plan.file.instructions.len(),
            built_outside_the_gate: cached.built_outside_the_gate,
        });
    }
    ping(daemon).await?;
    let tag = build_tag(&key);
    build(
        daemon,
        &DockerBuild {
            context_tar: context_tar(
                fs,
                where_from.context,
                &PinnedContainerfile {
                    path: &where_from.containerfile,
                    text: &pin_the_base(plan.text, preamble.line, &base.reference),
                },
            )?,
            containerfile: where_from.containerfile.clone(),
            build_args: executor::arg_defaults(plan.file)?,
            platform: format!("{}/{}", lns_artifact::image_index::OS, plan.arch),
            tag: tag.clone(),
            rebuild: plan.rebuild,
        },
    )
    .await?;
    let saved = read_saved_image(&export(daemon, &tag).await?)?;
    untag(daemon, &tag).await;
    let built = as_oci_image(&saved)?;
    let reference = host.adopt(&built, &saved.layers).await?;
    let outside_the_gate = Cached {
        reference,
        built_outside_the_gate: true,
    };
    host.remember(Kind::Image, &key, &outside_the_gate).await;
    Ok(Built {
        layers: saved.layers.len(),
        reference: outside_the_gate.reference,
        key,
        reused: false,
        reused_steps: 0,
        built_outside_the_gate: true,
    })
}

/// `DELETE /images/{name}`: the tag is lns's own leftover inside a daemon `lns sandbox prune` cannot see, so it goes as soon as the export is in hand; a daemon that keeps it is said so and stops nothing.
async fn untag<D: Daemon>(daemon: &D, tag: &str) {
    let kept = match call(
        daemon,
        ApiRequest::delete(&format!("/images/{tag}")).with("force", "1"),
    )
    .await
    {
        Ok(answer) if answer.status == 200 => return,
        Ok(answer) => format!("it answered {}", answer.status),
        Err(e) => format!("{e:#}"),
    };
    crate::log::warn!("the Docker daemon still holds its own copy of {tag}: {kept}");
}

/// The daemon resolves a `FROM` itself and may hold an older image behind the tag, so the file it is handed names the digest lns keyed the build over (§3.1.1).
pub(crate) fn pin_the_base(text: &str, from_line: usize, reference: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut dropping = false;
    for (index, line) in text.lines().enumerate() {
        if index + 1 == from_line {
            kept.push(format!("FROM {reference}"));
            dropping = continues_on_the_next_line(line);
            continue;
        }
        if dropping {
            dropping = continues_on_the_next_line(line);
            continue;
        }
        kept.push(line.to_string());
    }
    let mut pinned = kept.join("\n");
    if text.ends_with('\n') {
        pinned.push('\n');
    }
    pinned
}

fn continues_on_the_next_line(line: &str) -> bool {
    line.trim_end().ends_with('\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::context::tests::FakeContext;
    use std::cell::RefCell;

    struct FakeDaemon {
        answers: RefCell<Vec<Vec<u8>>>,
        sent: RefCell<Vec<String>>,
        socket: String,
        unreachable: bool,
    }

    impl FakeDaemon {
        fn answering(answers: &[&[u8]]) -> Self {
            Self {
                answers: RefCell::new(answers.iter().rev().map(|a| a.to_vec()).collect()),
                sent: RefCell::new(Vec::new()),
                socket: lns_ipc::DEFAULT_DOCKER_SOCKET.to_string(),
                unreachable: false,
            }
        }

        fn unreachable() -> Self {
            Self {
                answers: RefCell::new(Vec::new()),
                sent: RefCell::new(Vec::new()),
                socket: "/run/nowhere.sock".to_string(),
                unreachable: true,
            }
        }
    }

    impl Daemon for FakeDaemon {
        fn socket(&self) -> &str {
            &self.socket
        }

        async fn round_trip(&self, request: &[u8]) -> Result<Vec<u8>> {
            if self.unreachable {
                bail!("No such file or directory (os error 2)");
            }
            self.sent
                .borrow_mut()
                .push(String::from_utf8_lossy(request).to_string());
            self.answers
                .borrow_mut()
                .pop()
                .context("the fake daemon was asked more than it was scripted for")
        }
    }

    fn ok(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn a_request_carries_its_body_length_and_asks_the_daemon_to_close() {
        let bytes = encode(&ApiRequest::post(
            "/build",
            "application/x-tar",
            b"tar".to_vec(),
        ));
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("POST /v1.43/build HTTP/1.1\r\n"), "{text}");
        assert!(
            text.contains("Content-Type: application/x-tar\r\n"),
            "{text}"
        );
        assert!(text.contains("Content-Length: 3\r\n"), "{text}");
        assert!(text.contains("Connection: close\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\ntar"), "{text}");
    }

    #[test]
    fn every_query_value_is_escaped_so_a_build_argument_cannot_forge_one() {
        let bytes = encode(
            &ApiRequest::get("/build")
                .with("t", "lns-build.local/docker:aa")
                .with("buildargs", r#"{"V":"1 2&3"}"#),
        );
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.starts_with("GET /v1.43/build?t=lns-build.local%2Fdocker%3Aaa&buildargs=%7B%22V%22%3A%221+2%263%22%7D HTTP/1.1"),
            "{text}"
        );
    }

    #[test]
    fn a_chunked_answer_is_read_back_as_the_body_it_spells() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n3;x=1\r\n it\r\n0\r\n\r\n";
        let answer = decode(raw).expect("decoding");
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body, b"hello it");
    }

    #[test]
    fn a_plain_answer_is_read_back_whole_and_its_status_carried() {
        let answer = decode(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\nno").unwrap();
        assert_eq!((answer.status, answer.body), (404, b"no".to_vec()));
    }

    #[test]
    fn an_answer_that_is_no_http_message_is_refused_rather_than_read_as_empty() {
        assert!(decode(b"garbage").is_err());
        assert!(decode(b"garbage\r\n\r\n").is_err());
        assert!(
            decode(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhi").is_err(),
            "a chunk shorter than it declared is a truncated answer"
        );
        assert!(decode(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n").is_err());
        assert!(decode(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_err());
    }

    #[tokio::test]
    async fn a_daemon_that_does_not_answer_names_the_socket_and_the_switch_to_turn_off() {
        let err = ping(&FakeDaemon::unreachable()).await.unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("/run/nowhere.sock"), "{message}");
        assert!(message.contains("build.engine is docker"), "{message}");
        assert!(
            message.contains("lns config set build.engine lns"),
            "the refusal names the switch to turn off: {message}",
        );
    }

    #[tokio::test]
    async fn a_daemon_that_answers_a_ping_with_anything_but_200_is_no_daemon_to_build_on() {
        let daemon =
            FakeDaemon::answering(&[b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n"]);
        let err = ping(&daemon).await.unwrap_err();
        assert!(format!("{err:#}").contains("answered 503"), "{err:#}");
    }

    #[tokio::test]
    async fn a_ping_a_daemon_answers_is_the_engine_this_build_runs_on() {
        let daemon = FakeDaemon::answering(&[&ok("OK")]);
        ping(&daemon).await.expect("a daemon that answers");
        assert!(
            daemon.sent.borrow()[0].starts_with("GET /v1.43/_ping HTTP/1.1"),
            "{:?}",
            daemon.sent.borrow(),
        );
    }

    fn a_build() -> DockerBuild {
        DockerBuild {
            context_tar: b"tar bytes".to_vec(),
            containerfile: "Containerfile".to_string(),
            build_args: vec![("CLAUDE_CODE_VERSION".into(), "2.1.263".into())],
            platform: "linux/arm64".to_string(),
            tag: build_tag("sha256:aabb"),
            rebuild: false,
        }
    }

    #[tokio::test]
    async fn a_build_sends_the_context_the_file_inside_it_and_the_arg_defaults() {
        let daemon = FakeDaemon::answering(&[&ok(r#"{"stream":"Step 1/2"}"#)]);
        let tag = build(&daemon, &a_build()).await.expect("building");

        assert_eq!(tag, "lns-build.local/docker:aabb");
        let sent = daemon.sent.borrow()[0].clone();
        assert!(sent.starts_with("POST /v1.43/build?"), "{sent}");
        assert!(sent.contains("dockerfile=Containerfile"), "{sent}");
        assert!(sent.contains("platform=linux%2Farm64"), "{sent}");
        assert!(
            sent.contains("buildargs=%7B%22CLAUDE_CODE_VERSION%22%3A%222.1.263%22%7D"),
            "the ARG defaults the Containerfile declares are the build arguments: {sent}",
        );
        assert!(sent.contains("nocache=0"), "{sent}");
        assert!(sent.contains("Content-Type: application/x-tar"), "{sent}");
        assert!(sent.ends_with("tar bytes"), "{sent}");
    }

    #[tokio::test]
    async fn a_rebuild_tells_the_daemon_to_answer_from_no_cache_of_its_own() {
        let daemon = FakeDaemon::answering(&[&ok("{}")]);
        build(
            &daemon,
            &DockerBuild {
                rebuild: true,
                ..a_build()
            },
        )
        .await
        .expect("building");
        assert!(daemon.sent.borrow()[0].contains("nocache=1"));
    }

    /// The stream is not all JSON on every daemon, and a line lns cannot read is not a build failure.
    #[tokio::test]
    async fn a_build_the_daemon_failed_mid_stream_is_the_error_that_stream_named() {
        let daemon = FakeDaemon::answering(&[&ok(
            "not json at all\n{\"stream\":\"Step 1/2\"}\n{\"errorDetail\":{\"code\":1},\"error\":\"The command '/bin/sh -c npm i' returned a non-zero code: 1\"}\n",
        )]);
        let err = build(&daemon, &a_build()).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("returned a non-zero code: 1"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_build_the_daemon_refused_outright_names_the_status_it_refused_with() {
        let daemon = FakeDaemon::answering(&[
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\n\r\n{\"message\":\"no build\"}\n",
        ]);
        let err = build(&daemon, &a_build()).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the build with 400"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn an_export_asks_for_the_tag_the_build_wrote_and_answers_with_its_bytes() {
        let daemon = FakeDaemon::answering(&[&ok("tar")]);
        let bytes = export(&daemon, "lns-build.local/docker:aabb")
            .await
            .expect("exporting");
        assert_eq!(bytes, b"tar");
        assert!(
            daemon.sent.borrow()[0]
                .starts_with("GET /v1.43/images/lns-build.local/docker:aabb/get HTTP/1.1"),
            "{:?}",
            daemon.sent.borrow(),
        );
    }

    #[tokio::test]
    async fn an_image_the_daemon_will_not_export_names_the_tag_and_the_status() {
        let daemon =
            FakeDaemon::answering(&[b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope"]);
        let err = export(&daemon, "lns-build.local/docker:aabb")
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("lns-build.local/docker:aabb"), "{message}");
        assert!(message.contains("404"), "{message}");
    }

    #[tokio::test]
    async fn a_socket_that_answers_no_http_at_all_names_the_endpoint_it_was_asked() {
        let daemon = FakeDaemon::answering(&[b"not http"]);
        let err = ping(&daemon).await.unwrap_err();
        assert!(format!("{err:#}").contains("/v1.43/_ping"), "{err:#}");
    }

    fn saved_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            for (name, content) in entries {
                let mut header = entry_header(0o644, content.len() as u64, tar::EntryType::Regular);
                builder.append_data(&mut header, *name, *content).unwrap();
            }
            builder.finish().unwrap();
        }
        bytes
    }

    fn a_saved_image() -> Vec<u8> {
        saved_tar(&[
            (
                "manifest.json",
                br#"[{"Config":"blobs/sha256/cfg","RepoTags":["lns-build.local/docker:aabb"],"Layers":["blobs/sha256/one","blobs/sha256/two"]}]"#,
            ),
            (
                "blobs/sha256/cfg",
                br#"{"architecture":"arm64","os":"linux","config":{"Env":["PATH=/usr/bin"]}}"#,
            ),
            ("blobs/sha256/one", b"layer one"),
            ("blobs/sha256/two", b"layer two"),
        ])
    }

    #[test]
    fn an_exported_image_reads_back_as_its_config_and_every_layer_in_order() {
        let saved = read_saved_image(&a_saved_image()).expect("reading the export");
        assert!(saved.config.contains("\"architecture\":\"arm64\""));
        assert_eq!(
            saved
                .layers
                .iter()
                .map(|l| l.bytes.clone())
                .collect::<Vec<_>>(),
            vec![b"layer one".to_vec(), b"layer two".to_vec()],
            "the order the config stacks them in is the order the manifest names them",
        );
        assert_eq!(
            saved.layers[0].digest,
            format!("sha256:{}", hex::encode(Sha256::digest(b"layer one"))),
            "a layer is addressed by the bytes the daemon exported",
        );
    }

    #[test]
    fn an_export_missing_what_its_manifest_names_is_refused_rather_than_half_imported() {
        for (bytes, needle) in [
            (saved_tar(&[("hello", b"there")]), "no manifest.json"),
            (saved_tar(&[("manifest.json", b"[]")]), "names no image"),
            (
                saved_tar(&[("manifest.json", br#"[{"Layers":[]}]"#)]),
                "names no config blob",
            ),
            (
                saved_tar(&[("manifest.json", br#"[{"Config":"cfg"}]"#), ("cfg", b"{}")]),
                "names no layers",
            ),
            (
                saved_tar(&[
                    ("manifest.json", br#"[{"Config":"cfg","Layers":["gone"]}]"#),
                    ("cfg", b"{}"),
                ]),
                "names gone, which it does not carry",
            ),
            (
                saved_tar(&[
                    ("manifest.json", br#"[{"Config":"cfg","Layers":[7]}]"#),
                    ("cfg", b"{}"),
                ]),
                "names a layer that is no path",
            ),
            (
                saved_tar(&[("manifest.json", b"not json")]),
                "reading the exported image's manifest.json",
            ),
            (
                saved_tar(&[
                    ("manifest.json", br#"[{"Config":"cfg","Layers":[]}]"#),
                    ("cfg", &[0xff]),
                ]),
                "config is not text",
            ),
            (
                b"not a tar at all".to_vec(),
                "reading the image the Docker daemon exported",
            ),
        ] {
            let err = read_saved_image(&bytes).unwrap_err();
            assert!(
                format!("{err:#}").contains(needle),
                "wanted {needle}: {err:#}"
            );
        }
    }

    #[test]
    fn an_imported_image_is_addressed_the_way_every_other_built_image_is() {
        let saved = read_saved_image(&a_saved_image()).unwrap();
        let built = as_oci_image(&saved).expect("assembling");
        assert_eq!(built.config, saved.config);
        assert_eq!(built.manifest.layers.len(), 2);
        assert_eq!(
            built.manifest.layers[0].media_type,
            super::super::tar_layer::LAYER_MEDIA_TYPE,
            "an uncompressed export is an uncompressed layer",
        );
        assert_eq!(
            built.manifest.config.digest,
            format!(
                "sha256:{}",
                hex::encode(Sha256::digest(saved.config.as_bytes()))
            ),
        );
        assert_eq!(
            built.manifest_digest,
            format!(
                "sha256:{}",
                hex::encode(Sha256::digest(serde_json::to_vec(&built.manifest).unwrap()))
            ),
        );
    }

    #[test]
    fn a_daemon_that_exported_gzipped_layers_declares_them_as_gzipped() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, b"tar bytes").unwrap();
        let gzipped = gz.finish().unwrap();
        let saved = SavedImage {
            config: "{}".to_string(),
            layers: vec![LayerBlob {
                digest: "sha256:aa".into(),
                entries: 0,
                bytes: gzipped,
            }],
        };
        assert_eq!(
            as_oci_image(&saved).unwrap().manifest.layers[0].media_type,
            oci_client::manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE,
        );
    }

    #[test]
    fn an_export_with_no_layer_is_nothing_a_guest_could_boot() {
        let err = as_oci_image(&SavedImage {
            config: "{}".to_string(),
            layers: Vec::new(),
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("no layer"), "{err:#}");
    }

    /// The daemon pulls its own base, so the file it reads must name the digest the key was taken over rather than the tag that named it (§3.1.1).
    #[test]
    fn the_from_the_daemon_reads_is_the_digest_the_key_stands_on() {
        assert_eq!(
            pin_the_base(
                "ARG V=1.2.3\nFROM node:$V\nRUN npm i\n",
                2,
                "docker.io/library/node@sha256:base",
            ),
            "ARG V=1.2.3\nFROM docker.io/library/node@sha256:base\nRUN npm i\n",
        );
    }

    #[test]
    fn a_from_written_over_more_than_one_line_is_pinned_whole() {
        assert_eq!(
            pin_the_base("FROM \\\n  node:24\nRUN npm i\n", 1, "node@sha256:base"),
            "FROM node@sha256:base\nRUN npm i\n",
        );
    }

    #[test]
    fn a_file_that_ends_without_a_newline_is_pinned_without_one() {
        assert_eq!(
            pin_the_base("FROM node:24", 1, "node@sha256:base"),
            "FROM node@sha256:base",
        );
    }

    #[test]
    fn the_containerfile_the_daemon_reads_out_of_the_context_is_the_pinned_one() {
        let mut context = FakeContext::new();
        context
            .file("Containerfile", 0o644, b"FROM node:24\n")
            .file("app.js", 0o644, b"console.log(1)\n");

        let bytes = context_tar(
            &context,
            Path::new("/ctx"),
            &PinnedContainerfile {
                path: "Containerfile",
                text: "FROM node@sha256:base\n",
            },
        )
        .expect("packing");

        let held = tar_entries(&bytes).expect("reading the context back");
        assert_eq!(
            held.iter()
                .find(|(name, _)| name == "Containerfile")
                .map(|(_, bytes)| bytes.clone()),
            Some(b"FROM node@sha256:base\n".to_vec()),
        );
        assert_eq!(
            held.iter()
                .find(|(name, _)| name == "app.js")
                .map(|(_, bytes)| bytes.clone()),
            Some(b"console.log(1)\n".to_vec()),
            "every other file goes up as it is on the host",
        );
    }

    #[test]
    fn the_context_goes_up_as_a_tar_of_its_files_and_directories() {
        let mut context = FakeContext::new();
        context
            .file("Containerfile", 0o644, b"FROM alpine\n")
            .dir("app", 0o755)
            .file("app/main.js", 0o755, b"console.log(1)\n")
            .symlink("app/link", "main.js");

        let bytes = context_tar(&context, Path::new("/ctx"), &no_pin()).expect("packing");
        let mut archive = tar::Archive::new(std::io::Cursor::new(&bytes));
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["Containerfile", "app/", "app/main.js"],
            "a symlink is not part of the context, so the daemon never sees one",
        );
    }

    #[test]
    fn a_context_file_the_host_cannot_read_names_it_rather_than_sending_half_a_context() {
        let mut context = FakeContext::new();
        context
            .file("Containerfile", 0o644, b"FROM alpine\n")
            .unreadable_bytes("Containerfile");
        let err = context_tar(&context, Path::new("/ctx"), &no_pin()).unwrap_err();
        assert!(
            format!("{err:#}").contains("in the build context"),
            "{err:#}"
        );
    }

    #[test]
    fn an_entry_that_disappeared_between_the_listing_and_the_read_is_left_out() {
        let mut context = FakeContext::new();
        context
            .file("Containerfile", 0o644, b"FROM alpine\n")
            .dir("app", 0o755)
            .ghost("app", "gone");
        let bytes = context_tar(&context, Path::new("/ctx"), &no_pin()).expect("packing");
        let mut archive = tar::Archive::new(std::io::Cursor::new(&bytes));
        assert_eq!(archive.entries().unwrap().count(), 2);
    }

    #[test]
    fn a_context_directory_that_cannot_be_listed_names_it() {
        let mut context = FakeContext::new();
        context.dir("app", 0o755).unlistable("app");
        let err = context_tar(&context, Path::new("/ctx"), &no_pin()).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading the build context"),
            "{err:#}"
        );
    }

    #[test]
    fn a_tag_is_derived_from_the_key_so_two_builds_never_collide() {
        assert_eq!(build_tag("sha256:aabb"), "lns-build.local/docker:aabb");
        assert_eq!(build_tag("aabb"), "lns-build.local/docker:aabb");
    }

    #[derive(Default)]
    struct FakeDockerHost {
        base: Option<Base>,
        held: Option<Cached>,
        remembered: RefCell<Vec<(String, Cached)>>,
        adopted: RefCell<Vec<(String, usize)>>,
    }

    impl DockerHost for FakeDockerHost {
        async fn peek_base(&self, image: &str) -> Result<Base> {
            self.base
                .clone()
                .with_context(|| format!("no registry answers for {image}"))
        }

        async fn cached(&self, _kind: Kind, _key: &str) -> Option<Cached> {
            self.held.clone()
        }

        async fn remember(&self, _kind: Kind, key: &str, built: &Cached) {
            self.remembered
                .borrow_mut()
                .push((key.to_string(), built.clone()));
        }

        async fn adopt(&self, built: &BuiltImage, layers: &[LayerBlob]) -> Result<String> {
            self.adopted
                .borrow_mut()
                .push((built.manifest_digest.clone(), layers.len()));
            Ok(format!("lns-build.local/built@{}", built.manifest_digest))
        }
    }

    fn a_host() -> FakeDockerHost {
        FakeDockerHost {
            base: Some(Base {
                reference: "docker.io/library/node@sha256:base".to_string(),
                env: Vec::new(),
            }),
            ..FakeDockerHost::default()
        }
    }

    const TEXT: &str = "ARG V=1.2.3\nFROM docker.io/library/node:24\nRUN npm i -g claude@$V\n";

    async fn built_through(
        host: &FakeDockerHost,
        daemon: &FakeDaemon,
        rebuild: bool,
    ) -> Result<Built> {
        let file = lns_artifact::containerfile::parse(TEXT).expect("a Containerfile lns builds");
        let mut context = FakeContext::new();
        context.file("Containerfile", 0o644, TEXT.as_bytes());
        build_image(
            host,
            daemon,
            &context,
            &BuildPlan {
                file: &file,
                label: "./image/Containerfile",
                text: TEXT,
                context_hash: "sha256:ctx",
                arch: "arm64",
                rebuild,
                policy: "sha256:policy",
            },
            &DaemonBuild {
                context: Path::new("/ctx"),
                containerfile: "Containerfile".to_string(),
            },
        )
        .await
    }

    fn a_daemon_that_builds() -> FakeDaemon {
        FakeDaemon::answering(&[
            &ok("OK"),
            &ok(r#"{"stream":"Successfully built"}"#),
            &ok_bytes(&a_saved_image()),
            &ok("[]"),
        ])
    }

    fn no_pin() -> PinnedContainerfile<'static> {
        PinnedContainerfile { path: "", text: "" }
    }

    fn ok_bytes(body: &[u8]) -> Vec<u8> {
        let mut bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    #[tokio::test]
    async fn a_daemon_build_lands_in_the_local_store_under_the_key_the_lns_path_would_use() {
        let host = a_host();
        let daemon = a_daemon_that_builds();
        let built = built_through(&host, &daemon, false)
            .await
            .expect("building");

        assert!(!built.reused);
        assert_eq!(
            built.layers, 2,
            "every layer the export carried is imported"
        );
        assert_eq!(
            built.key,
            super::super::key::image_key(
                "docker.io/library/node@sha256:base",
                TEXT,
                "sha256:ctx",
                "arm64",
            ),
            "the key is the one the build guest would answer, so slice 4's cache is unchanged",
        );
        assert_eq!(
            host.remembered.borrow().first().map(|(key, _)| key.clone()),
            Some(built.key.clone()),
        );
        assert_eq!(host.adopted.borrow().len(), 1);
        assert_eq!(built.reference, host.remembered.borrow()[0].1.reference);
        assert_eq!(
            daemon.sent.borrow().len(),
            4,
            "a ping, a build, an export and the removal of the tag, and nothing else",
        );
        assert!(
            daemon.sent.borrow()[3].starts_with("DELETE /v1.43/images/lns-build.local%2Fdocker%3A")
                || daemon.sent.borrow()[3]
                    .starts_with("DELETE /v1.43/images/lns-build.local/docker:"),
            "the daemon's own copy is not left behind: {}",
            daemon.sent.borrow()[3],
        );
        assert!(
            daemon.sent.borrow()[1].contains("platform=linux%2Farm64"),
            "the daemon is asked for the platform the key was taken over",
        );
        assert!(
            daemon.sent.borrow()[1].contains("buildargs=%7B%22V%22%3A%221.2.3%22%7D"),
            "the ARG defaults are stated rather than left to the daemon",
        );
        assert!(
            daemon.sent.borrow()[1].contains("FROM docker.io/library/node@sha256:base"),
            "the daemon builds on the digest the key was taken over, not on the tag: {}",
            daemon.sent.borrow()[1],
        );
    }

    /// Nothing else on this machine records which engine filled a key, so the entry the reuse answers with is where the disclosure comes from (§3.1.1).
    #[tokio::test]
    async fn a_daemon_build_is_remembered_as_one_the_gate_did_not_apply_to() {
        let host = a_host();
        let built = built_through(&host, &a_daemon_that_builds(), false)
            .await
            .expect("building");

        assert!(built.built_outside_the_gate);
        assert!(
            host.remembered.borrow()[0].1.built_outside_the_gate,
            "the next build to answer this key has to read the daemon off it",
        );
    }

    /// The switch says `docker`, but this key was filled in a build guest, so the gate did apply to the image it answers with.
    #[tokio::test]
    async fn a_key_a_guest_filled_answers_the_daemon_switch_and_says_the_gate_applied() {
        let host = FakeDockerHost {
            held: Some(Cached {
                reference: "lns-build.local/built@sha256:held".to_string(),
                built_outside_the_gate: false,
            }),
            ..a_host()
        };
        let built = built_through(&host, &FakeDaemon::answering(&[]), false)
            .await
            .expect("building");

        assert!(built.reused);
        assert!(!built.built_outside_the_gate);
    }

    #[tokio::test]
    async fn a_key_this_machine_already_answers_runs_no_daemon_at_all() {
        let host = FakeDockerHost {
            held: Some(Cached {
                reference: "lns-build.local/built@sha256:held".to_string(),
                built_outside_the_gate: false,
            }),
            ..a_host()
        };
        let daemon = FakeDaemon::answering(&[]);
        let built = built_through(&host, &daemon, false)
            .await
            .expect("building");

        assert!(built.reused);
        assert_eq!(built.reference, "lns-build.local/built@sha256:held");
        assert!(
            daemon.sent.borrow().is_empty(),
            "a build the cache answers never reaches the daemon",
        );
    }

    #[tokio::test]
    async fn a_rebuild_ignores_the_key_this_machine_holds_and_builds_again() {
        let host = FakeDockerHost {
            held: Some(Cached {
                reference: "lns-build.local/built@sha256:held".to_string(),
                built_outside_the_gate: false,
            }),
            ..a_host()
        };
        let built = built_through(&host, &a_daemon_that_builds(), true)
            .await
            .expect("building");
        assert!(!built.reused);
    }

    /// The image is in lns's layer store by then, so a daemon that will not drop its copy is said so and the build stands.
    #[test]
    fn a_daemon_that_will_not_drop_its_copy_says_so_and_finishes_the_build() {
        let daemon = FakeDaemon::answering(&[
            &ok("OK"),
            &ok(r#"{"stream":"Successfully built"}"#),
            &ok_bytes(&a_saved_image()),
            &b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ]);
        let mut built = None;
        let messages = crate::test_env::captured_messages(|| {
            built = Some(
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("a runtime for one build")
                    .block_on(built_through(&a_host(), &daemon, false)),
            );
        });

        assert!(built.expect("the build was run").is_ok());
        assert!(
            messages
                .iter()
                .any(|m| m.contains("still holds its own copy")),
            "{messages:?}",
        );
    }

    #[tokio::test]
    async fn a_daemon_that_does_not_answer_stops_the_build_before_the_context_is_sent() {
        let daemon = FakeDaemon::unreachable();
        let err = built_through(&a_host(), &daemon, false).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("no Docker daemon answered"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_base_no_registry_answers_for_is_refused_naming_the_from_line() {
        let err = built_through(&FakeDockerHost::default(), &a_daemon_that_builds(), false)
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("line 2: FROM docker.io/library/node:24"),
            "{message}"
        );
    }
}
