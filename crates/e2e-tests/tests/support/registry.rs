//! A minimal in-process OCI distribution registry for the bundle-boot e2e.
//!
//! Serves exactly the subset `lns build --push` (chunked blob upload +
//! manifest PUT) and `lns run` (manifest/blob GET) exercise, over plaintext
//! HTTP on a loopback port. Not a general registry — no auth, no GC, in-memory.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Store {
    blobs: HashMap<String, Vec<u8>>,
    manifests: HashMap<(String, String), Manifest>,
    uploads: HashMap<String, Vec<u8>>,
    next_upload: u64,
}

#[derive(Clone)]
struct Manifest {
    bytes: Vec<u8>,
    content_type: String,
    digest: String,
}

#[derive(Debug)]
pub struct LocalRegistry {
    port: u16,
}

impl LocalRegistry {
    /// Bind a loopback port and serve requests on a detached thread for the life of the process.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local registry");
        let port = listener.local_addr().expect("registry addr").port();
        let store = Arc::new(Mutex::new(Store::default()));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let store = store.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &store);
                });
            }
        });
        Self { port }
    }

    /// The `host:port` a ref should target (e.g. `127.0.0.1:5000/some/repo:1`).
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(Request {
        method,
        path,
        query,
        headers,
        body,
    }))
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Split `/v2/<name>/(blobs|manifests|...)` into (name, tail-segments after the marker).
fn route(path: &str) -> Option<(String, Vec<String>)> {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    if segs.first() != Some(&"v2") {
        return None;
    }
    let marker = segs
        .iter()
        .position(|s| *s == "blobs" || *s == "manifests")?;
    let name = segs[1..marker].join("/");
    let tail = segs[marker..].iter().map(|s| s.to_string()).collect();
    Some((name, tail))
}

fn serve(mut stream: TcpStream, store: &Arc<Mutex<Store>>) -> std::io::Result<()> {
    let Some(req) = read_request(&mut stream)? else {
        return Ok(());
    };
    // Version check.
    if req.path == "/v2" || req.path == "/v2/" {
        return respond(&mut stream, "200 OK", &[], b"{}");
    }
    let Some((name, tail)) = route(&req.path) else {
        return respond(&mut stream, "404 Not Found", &[], b"not found");
    };
    let tail: Vec<&str> = tail.iter().map(String::as_str).collect();
    match (req.method.as_str(), tail.as_slice()) {
        ("POST", ["blobs", "uploads"]) | ("POST", ["blobs", "uploads", _]) => {
            let uuid = {
                let mut s = store.lock().unwrap();
                s.next_upload += 1;
                let uuid = format!("u{}", s.next_upload);
                s.uploads.insert(uuid.clone(), Vec::new());
                uuid
            };
            let location = format!("/v2/{name}/blobs/uploads/{uuid}");
            respond(
                &mut stream,
                "202 Accepted",
                &[("Location", &location), ("Range", "0-0")],
                b"",
            )
        }
        ("PATCH", ["blobs", "uploads", uuid]) => {
            let end = {
                let mut s = store.lock().unwrap();
                let buf = s.uploads.entry(uuid.to_string()).or_default();
                buf.extend_from_slice(&req.body);
                buf.len()
            };
            let location = format!("/v2/{name}/blobs/uploads/{uuid}");
            let range = format!("0-{}", end.saturating_sub(1));
            respond(
                &mut stream,
                "202 Accepted",
                &[("Location", &location), ("Range", &range)],
                b"",
            )
        }
        ("PUT", ["blobs", "uploads", uuid]) => {
            let digest = query_param(&req.query, "digest")
                .map(|d| percent_decode(&d))
                .unwrap_or_default();
            {
                let mut s = store.lock().unwrap();
                let mut buf = s.uploads.remove(*uuid).unwrap_or_default();
                buf.extend_from_slice(&req.body);
                s.blobs.insert(digest.clone(), buf);
            }
            let location = format!("/v2/{name}/blobs/{digest}");
            respond(
                &mut stream,
                "201 Created",
                &[("Location", &location), ("Docker-Content-Digest", &digest)],
                b"",
            )
        }
        ("HEAD", ["blobs", digest]) | ("GET", ["blobs", digest]) => {
            let blob = store.lock().unwrap().blobs.get(*digest).cloned();
            match blob {
                Some(bytes) => {
                    let body = if req.method == "HEAD" {
                        &[][..]
                    } else {
                        &bytes[..]
                    };
                    let len = bytes.len().to_string();
                    respond(
                        &mut stream,
                        "200 OK",
                        &[
                            ("Docker-Content-Digest", digest),
                            ("Content-Type", "application/octet-stream"),
                            ("X-Content-Length", &len),
                        ],
                        body,
                    )
                }
                None => respond(&mut stream, "404 Not Found", &[], b"no blob"),
            }
        }
        ("PUT", ["manifests", reference]) => {
            let digest = digest_of(&req.body);
            let content_type = req
                .headers
                .get("content-type")
                .cloned()
                .unwrap_or_else(|| "application/vnd.oci.image.manifest.v1+json".to_string());
            let manifest = Manifest {
                bytes: req.body.clone(),
                content_type,
                digest: digest.clone(),
            };
            {
                let mut s = store.lock().unwrap();
                s.manifests
                    .insert((name.clone(), reference.to_string()), manifest.clone());
                s.manifests.insert((name.clone(), digest.clone()), manifest);
            }
            let location = format!("/v2/{name}/manifests/{digest}");
            respond(
                &mut stream,
                "201 Created",
                &[("Location", &location), ("Docker-Content-Digest", &digest)],
                b"",
            )
        }
        ("HEAD", ["manifests", reference]) | ("GET", ["manifests", reference]) => {
            let found = store
                .lock()
                .unwrap()
                .manifests
                .get(&(name.clone(), reference.to_string()))
                .cloned();
            match found {
                Some(manifest) => {
                    let body = if req.method == "HEAD" {
                        &[][..]
                    } else {
                        &manifest.bytes[..]
                    };
                    respond(
                        &mut stream,
                        "200 OK",
                        &[
                            ("Content-Type", &manifest.content_type),
                            ("Docker-Content-Digest", &manifest.digest),
                        ],
                        body,
                    )
                }
                None => respond(&mut stream, "404 Not Found", &[], b"no manifest"),
            }
        }
        _ => respond(&mut stream, "404 Not Found", &[], b"unhandled"),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(byte as char);
            i += 3;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    })
}
