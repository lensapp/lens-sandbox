use super::{FixtureReport, HARNESS_VERSION, Shared};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const ACCEPT_POLL: Duration = Duration::from_millis(200);
const HEAD_LIMIT: usize = 8 * 1024;

/// What a runner on another machine checks before it starts: the version both halves must share, and the fixtures this process actually bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub harness_version: String,
    pub bind: String,
    pub ports: BTreeMap<String, u16>,
    pub fixtures: Vec<String>,
}

pub(super) fn spawn(shared: Arc<Shared>, addr: SocketAddrV4) -> Result<u16> {
    let listener = std::net::TcpListener::bind(addr)
        .with_context(|| format!("bind the report server on {addr}"))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        runtime.block_on(accept_loop(listener, shared));
    });
    Ok(port)
}

async fn accept_loop(listener: std::net::TcpListener, shared: Arc<Shared>) {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
        return;
    };
    while !shared.stopping() {
        match tokio::time::timeout(ACCEPT_POLL, listener.accept()).await {
            Ok(Ok((stream, _))) => {
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let _ = serve(stream, shared).await;
                });
            }
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let head = read_head(&mut stream).await?;
    let (method, path) = request_line(&head);
    let response = respond(&method, &path, &shared);
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < HEAD_LIMIT {
        if stream.read(&mut byte).await? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn request_line(head: &str) -> (String, String) {
    let mut tokens = head.lines().next().unwrap_or_default().split_whitespace();
    let method = tokens.next().unwrap_or_default().to_string();
    let path = tokens.next().unwrap_or_default().to_string();
    (method, path)
}

fn respond(method: &str, path: &str, shared: &Shared) -> String {
    let (status, body) = match (method, path) {
        ("GET", "/health") => (200, json(&health(shared))),
        ("GET", "/report") => (200, json(&shared.with_report(|report| report.clone()))),
        ("POST", "/reset") => {
            shared.reset();
            (200, "{\"reset\":true}".to_string())
        }
        (_, "/health" | "/report" | "/reset") => (
            405,
            error_body(&format!("{method} is not how this route is asked")),
        ),
        _ => (
            404,
            error_body("this server serves /health, /report and /reset"),
        ),
    };
    http_response(status, &body)
}

fn health(shared: &Shared) -> Health {
    let report: FixtureReport = shared.with_report(|report| report.clone());
    Health {
        harness_version: HARNESS_VERSION.to_string(),
        bind: report.bind.clone(),
        fixtures: report.ports.keys().cloned().collect(),
        ports: report.ports,
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|err| error_body(&format!("the report did not serialise: {err}")))
}

fn error_body(message: &str) -> String {
    json(&BTreeMap::from([("error", message)]))
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::http::request;
    use crate::fixtures::{Fixtures, Role, Sizes};
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpStream as StdStream};

    fn served() -> (Fixtures, SocketAddrV4) {
        let fixtures = Fixtures::start(Ipv4Addr::LOCALHOST, 0, Sizes::default())
            .expect("the fixtures bind on loopback");
        let port = fixtures.serve_report(0).expect("the report server binds");
        (fixtures, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn health_names_the_version_both_halves_must_share_and_the_fixtures_it_bound() {
        let (fixtures, endpoint) = served();
        let (status, body) = request(endpoint, "GET", "/health").unwrap();
        let health: Health = serde_json::from_str(&body).expect("health parses");

        assert_eq!(status, 200);
        assert_eq!(health.harness_version, HARNESS_VERSION);
        assert_eq!(health.bind, "127.0.0.1");
        assert_eq!(health.ports, fixtures.report().ports);
        assert!(health.fixtures.contains(&"echo".to_string()), "{health:?}");
    }

    #[test]
    fn the_report_route_carries_what_the_fixtures_saw() {
        let (fixtures, endpoint) = served();
        let mut client = StdStream::connect((fixtures.bind(), fixtures.port(Role::Sink))).unwrap();
        client.write_all(&[0u8; 512]).unwrap();
        drop(client);

        let report = crate::cases::poll(Duration::from_secs(5), || {
            let (_, body) = request(endpoint, "GET", "/report").ok()?;
            let report: FixtureReport = serde_json::from_str(&body).ok()?;
            (!report.connections.is_empty()).then_some(report)
        })
        .expect("the served report shows the connection");

        assert_eq!(report.bind, "127.0.0.1");
        assert_eq!(report.connections[0].bytes_in, 512);
    }

    #[test]
    fn reset_clears_the_counters_between_two_cases() {
        let (fixtures, endpoint) = served();
        let mut client = StdStream::connect((fixtures.bind(), fixtures.port(Role::Sink))).unwrap();
        client.write_all(&[0u8; 16]).unwrap();
        drop(client);
        crate::cases::poll(Duration::from_secs(5), || {
            (!fixtures.report().connections.is_empty()).then_some(())
        })
        .expect("the fixture recorded the connection");

        let (status, body) = request(endpoint, "POST", "/reset").unwrap();

        assert_eq!(status, 200);
        assert_eq!(body, "{\"reset\":true}");
        assert!(fixtures.report().connections.is_empty());
        assert_eq!(fixtures.report().witness_accepts, 0);
    }

    #[test]
    fn a_route_this_server_does_not_serve_is_refused_and_says_what_it_does_serve() {
        let (_fixtures, endpoint) = served();
        let (status, body) = request(endpoint, "GET", "/cases").unwrap();
        assert_eq!(status, 404);
        assert!(body.contains("/report"), "{body}");

        let (status, _) = request(endpoint, "GET", "/reset").unwrap();
        assert_eq!(status, 405);
    }

    #[test]
    fn a_request_with_no_request_line_is_answered_rather_than_dropped() {
        assert_eq!(request_line(""), (String::new(), String::new()));
        assert_eq!(
            request_line("GET /report HTTP/1.1\r\n\r\n"),
            ("GET".to_string(), "/report".to_string())
        );
    }
}
