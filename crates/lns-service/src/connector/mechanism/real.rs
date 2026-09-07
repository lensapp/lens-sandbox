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
