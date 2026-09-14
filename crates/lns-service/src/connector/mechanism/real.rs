//! Production wiring for a mechanism: one component runtime for this machine,
//! and the four things lns lends a component, each doing the real thing.

use std::sync::Arc;

use anyhow::Result;

use super::host::Host;
use super::traits::{Entropy, Exec, Http, Recorder};
use super::wasm::Runtime;
use super::{Bounds, CallError, ExecOutput, HttpRequest, HttpResponse};
use crate::connector::session::Sessions as _;

/// The engine every component on this machine runs in, and the thread that spends their deadlines.
pub struct RealMechanisms {
    pub(super) runtime: Runtime,
    http: Arc<dyn Http>,
    exec: Arc<dyn Exec>,
    entropy: Arc<dyn Entropy>,
    recorder: Arc<dyn Recorder>,
    browser: Arc<RealBrowser>,
}

impl RealMechanisms {
    pub fn new() -> Result<Self> {
        Self::lending(
            Arc::new(RealHttp),
            Arc::new(RealExec),
            Arc::new(RealEntropy),
            Arc::new(RealRecorder),
        )
    }

    /// The engine, lending whatever it is given. A test builds one that reaches nothing, so a layer that forbids the network does not hold the capability either.
    pub fn lending(
        http: Arc<dyn Http>,
        exec: Arc<dyn Exec>,
        entropy: Arc<dyn Entropy>,
        recorder: Arc<dyn Recorder>,
    ) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new()?,
            http,
            exec,
            entropy,
            recorder,
            browser: Arc::new(RealBrowser::default()),
        })
    }

    /// Spends one second of every running component's deadline, and sweeps what nobody came back for. This thread does nothing else: whatever ran a component here would stop the clock its own deadline is measured in.
    pub fn tick(self: &Arc<Self>) {
        let ticking = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                ticking.runtime.tick_once();
                sessions().sweep(now_millis());
            }
        });
    }
}

impl RealMechanisms {
    pub(super) fn host(&self, connector: &str, bounds: Bounds) -> Host {
        Host::new(
            connector,
            bounds,
            Arc::clone(&self.http),
            Arc::clone(&self.exec),
            Arc::clone(&self.entropy),
            Arc::clone(&self.recorder),
        )
        .with_browser(self.browser.clone())
    }
}

/// Runs synchronous work without holding a runtime worker. A current-thread runtime has no other worker to step onto, so there it runs in place — which is what a test harness gives it.
pub fn off_the_runtime_thread<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The most one response may bring into this machine's memory. A token exchange answers in kilobytes.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

struct RealHttp;

impl RealHttp {
    /// One client for this machine, and it follows nothing: a redirect is a host the server chose, and lns bounds only the ones the method declared, so a 3xx goes back to the component to decide about (§3.2.6).
    fn client() -> Result<&'static reqwest::Client, CallError> {
        static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .ok()
            })
            .as_ref()
            .ok_or_else(|| CallError::Failed("this machine has no http client".to_string()))
    }
}

impl Http for RealHttp {
    /// A mechanism call is synchronous and arrives on a runtime thread, so this steps off it before blocking: `block_on` alone would panic there.
    fn fetch(
        &self,
        request: &HttpRequest,
        within: std::time::Duration,
    ) -> Result<HttpResponse, CallError> {
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| CallError::Failed(format!("no runtime to reach a host with: {e}")))?;
        off_the_runtime_thread(|| {
            handle.block_on(async {
                let client = Self::client()?;
                let method = reqwest::Method::from_bytes(request.method.as_bytes())
                    .map_err(|e| CallError::Failed(format!("{e}")))?;
                let mut sending = client.request(method, &request.url);
                for (name, value) in &request.headers {
                    sending = sending.header(name, value);
                }
                let mut response = sending
                    .body(request.body.clone())
                    .timeout(within)
                    .send()
                    .await
                    .map_err(|e| CallError::Failed(format!("{e}")))?;
                let status = response.status().as_u16();
                let headers = response
                    .headers()
                    .iter()
                    .filter_map(|(name, value)| {
                        Some((name.to_string(), value.to_str().ok()?.to_string()))
                    })
                    .collect();
                let mut body = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|e| CallError::Failed(format!("{e}")))?
                {
                    if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                        return Err(CallError::Failed(format!(
                            "that host answered with more than {MAX_RESPONSE_BYTES} bytes"
                        )));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(HttpResponse {
                    status,
                    headers,
                    body,
                })
            })
        })
    }
}

struct RealExec;

impl Exec for RealExec {
    /// A mechanism call is synchronous and arrives on a runtime thread, so this steps off it before waiting.
    fn run(&self, argv: &[String], within: std::time::Duration) -> Result<ExecOutput, CallError> {
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| CallError::Refused("a program to run was not named".to_string()))?;
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| CallError::Failed(format!("no runtime to run a program on: {e}")))?;
        off_the_runtime_thread(|| {
            handle.block_on(async {
                let started = tokio::process::Command::new(program)
                    .args(arguments)
                    // lns bounds nothing such a program reaches, but it need not leave one running after it stopped waiting.
                    .kill_on_drop(true)
                    .output();
                let output = tokio::time::timeout(within, started)
                    .await
                    .map_err(|_| {
                        CallError::Failed(format!(
                            "{program} did not finish within {} seconds",
                            within.as_secs()
                        ))
                    })?
                    .map_err(|e| CallError::Failed(format!("{program}: {e}")))?;
                Ok(ExecOutput {
                    code: output.status.code().unwrap_or(-1),
                    stdout: output.stdout,
                    stderr: output.stderr,
                })
            })
        })
    }
}

struct RealEntropy;

impl Entropy for RealEntropy {
    fn bytes(&self, count: u32) -> Vec<u8> {
        use rand::RngCore;
        let mut drawn = vec![0u8; count as usize];
        rand::thread_rng().fill_bytes(&mut drawn);
        drawn
    }
}

pub struct RealRecorder;

impl Recorder for RealRecorder {
    fn reached(&self, connector: &str, host: &str, refused: bool) {
        wrote(connector, "reached", host, refused);
    }

    fn ran(&self, connector: &str, program: &str, refused: bool) {
        wrote(connector, "ran", program, refused);
    }

    fn renewed(&self, connector: &str, target: &str, refused: bool) {
        wrote(connector, "renewed", target, refused);
    }

    fn elided(&self, connector: &str, after: u32) {
        wrote(
            connector,
            "elided",
            &format!("the rest of this call after {after} entries"),
            false,
        );
    }
}

/// §3.2.6 makes every execution and every outbound call a durable record; failing to write one must not fail the connect it describes.
fn wrote(connector: &str, verb: &str, target: &str, refused: bool) {
    if let Err(e) = crate::ledger::append_mechanism_event(
        &crate::clock::RealClock,
        connector,
        verb,
        target,
        refused,
    ) {
        crate::log::warn!("could not record what {connector} {verb}: {e:#}");
    }
}

/// The engine this machine runs components in, as an `Arc` the schedule can hold.
pub fn shared() -> Result<Arc<RealMechanisms>> {
    mechanisms()?;
    ENGINE
        .get()
        .and_then(Clone::clone)
        .ok_or_else(|| anyhow::anyhow!("this machine could not start a component runtime"))
}

static ENGINE: std::sync::OnceLock<Option<Arc<RealMechanisms>>> = std::sync::OnceLock::new();

/// The engine, the four things lns lends a component, and the connects this process is part-way through. One of each, because a component runtime is expensive and a session outlives the request that opened it.
pub fn mechanisms() -> Result<&'static RealMechanisms> {
    ENGINE
        .get_or_init(|| {
            let started = RealMechanisms::new().map(std::sync::Arc::new);
            if let Err(e) = &started {
                crate::log::warn!("the component runtime could not start: {e:#}");
            }
            let started = started.ok()?;
            started.tick();
            Some(started)
        })
        .as_deref()
        .ok_or_else(|| {
            anyhow::anyhow!("this machine could not start a component runtime, so no connector that carries its own mechanism can be connected")
        })
}

pub fn sessions() -> &'static crate::connector::session::InMemorySessions {
    static OPEN: std::sync::OnceLock<crate::connector::session::InMemorySessions> =
        std::sync::OnceLock::new();
    OPEN.get_or_init(crate::connector::session::InMemorySessions::default)
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// What a connect on this machine is driven with.
pub fn driver<'a>(
    store: crate::connector::store::ConnectorStore<'a>,
) -> Result<crate::connector::connect::Driver<'a>> {
    Ok(crate::connector::connect::Driver {
        store,
        mechanisms: mechanisms()?,
        sessions: sessions(),
        now_millis: now_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_with_no_runtime_reaches_nothing_and_runs_nothing() {
        // The refresh pass used to run on such a thread, which made every renewal that calls a provider fail before it left the machine.
        let refused = RealHttp
            .fetch(
                &HttpRequest {
                    method: "GET".to_string(),
                    url: "https://auth.example.com/token".to_string(),
                    headers: Vec::new(),
                    body: Vec::new(),
                },
                std::time::Duration::from_secs(5),
            )
            .expect_err("there is no runtime here");

        assert!(
            matches!(refused, CallError::Failed(ref why) if why.contains("no runtime")),
            "{refused:?}"
        );
        assert!(matches!(
            RealExec
                .run(&["true".to_string()], std::time::Duration::from_secs(5))
                .expect_err("there is no runtime here"),
            CallError::Failed(_)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reaching_a_host_from_a_runtime_thread_answers_rather_than_panicking() {
        // A mechanism call is synchronous and arrives on a runtime thread; blocking there without stepping off it aborts the request.
        let refused = RealHttp
            .fetch(
                &HttpRequest {
                    method: "GET".to_string(),
                    // Nothing listens, so this fails without leaving the machine.
                    url: "https://127.0.0.1:1/token".to_string(),
                    headers: Vec::new(),
                    body: Vec::new(),
                },
                std::time::Duration::from_secs(5),
            )
            .expect_err("nothing is listening there");

        assert!(matches!(refused, CallError::Failed(_)), "{refused:?}");
    }
}

#[derive(Default)]
struct RealBrowser {
    flows: Arc<std::sync::Mutex<super::browser::Flows<tokio::net::TcpListener>>>,
}

impl RealBrowser {
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, super::browser::Flows<tokio::net::TcpListener>>, CallError>
    {
        self.flows
            .lock()
            .map_err(|_| CallError::Failed("browser authorization state is unavailable".into()))
    }
}

impl super::traits::Browser for RealBrowser {
    fn prepare(
        &self,
        connector: &str,
        within: std::time::Duration,
    ) -> Result<super::traits::BrowserSession, CallError> {
        let mut flows = self.lock()?;
        let now = now_millis();
        flows.capacity(connector, now)?;
        let listener =
            std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|_| {
                CallError::Failed("could not listen for browser authorization on loopback".into())
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|_| CallError::Failed("could not prepare browser callback listener".into()))?;
        let port = listener
            .local_addr()
            .map_err(|_| CallError::Failed("could not read browser callback address".into()))?
            .port();
        let listener = tokio::net::TcpListener::from_std(listener)
            .map_err(|_| CallError::Failed("could not start browser callback listener".into()))?;
        let session = super::traits::BrowserSession {
            handle: uuid::Uuid::new_v4().to_string(),
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
            state: uuid::Uuid::new_v4().to_string(),
        };
        let within = within.min(std::time::Duration::from_secs(900));
        flows.insert(
            connector,
            &session,
            listener,
            now.saturating_add(within.as_millis() as u64),
        );
        let retained = self.flows.clone();
        let expiring = session.handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(within).await;
            if let Ok(mut flows) = retained.lock() {
                flows.remove(&expiring);
            }
        });
        Ok(session)
    }

    fn open(&self, connector: &str, handle: &str, url: &str) -> Result<(), CallError> {
        let (listener, state, remaining) =
            self.lock()?.open(connector, handle, url, now_millis())?;
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let child = tokio::process::Command::new(opener)
            .kill_on_drop(true)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(_) => {
                self.lock()?.remove(handle);
                return Err(CallError::Failed(
                    "could not open the browser for authorization".into(),
                ));
            }
        };
        let handle = handle.to_string();
        let retained = self.flows.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(std::time::Duration::from_millis(remaining), async {
                let status = child.wait().await.map_err(|_| {
                    CallError::Failed("could not wait for the browser opener".into())
                })?;
                if !status.success() {
                    return Err(CallError::Failed("the browser could not be opened".into()));
                }
                receive_browser_code(listener, &state).await
            })
            .await
            .unwrap_or_else(|_| {
                Err(CallError::Failed(
                    "browser authorization expired; connect again".into(),
                ))
            });
            if let Ok(mut flows) = retained.lock() {
                flows.complete(&handle, result);
            }
        });
        Ok(())
    }

    fn poll(&self, connector: &str, handle: &str) -> Result<Option<String>, CallError> {
        self.lock()?.poll(connector, handle, now_millis())
    }
}

async fn receive_browser_code(
    listener: tokio::net::TcpListener,
    state: &str,
) -> Result<String, CallError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| CallError::Failed("browser callback listener failed".into()))?;
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_browser_callback(&mut stream, state),
        )
        .await;
        match attempt {
            Ok(Ok(super::browser::Callback::Code(code))) => return Ok(code),
            Ok(Ok(super::browser::Callback::Denied)) => {
                return Err(CallError::Failed(
                    "browser authorization was declined".into(),
                ));
            }
            _ => {}
        }
    }
}

async fn read_browser_callback(
    stream: &mut tokio::net::TcpStream,
    state: &str,
) -> std::io::Result<super::browser::Callback> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 4096 {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let callback = if bytes.len() <= 4096 && bytes.ends_with(b"\r\n\r\n") {
        std::str::from_utf8(&bytes)
            .map(|request| super::browser::callback(request, state))
            .unwrap_or(super::browser::Callback::Ignore)
    } else {
        super::browser::Callback::Ignore
    };
    let (status, message) = match callback {
        super::browser::Callback::Code(_) => (
            "200 OK",
            "Authorization received. Return to LNS and continue connecting.",
        ),
        super::browser::Callback::Denied => ("200 OK", "Authorization declined. Return to LNS."),
        super::browser::Callback::Ignore => (
            "400 Bad Request",
            "This request does not match an open LNS authorization.",
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(callback)
}
