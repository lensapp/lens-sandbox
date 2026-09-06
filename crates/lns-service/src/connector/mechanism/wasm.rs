//! A `code` method's mechanism, run as a WebAssembly component: lns cannot read what it does, so what stands in for reading it is what [`lend_the_standard_library`] hands over and what [`Host`] bounds (§3.2.6).

use anyhow::{Result, anyhow};
use wasmtime::component::{Component as WasmComponent, Linker, ResourceTable};
use wasmtime::{Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::cli::{WasiCli, WasiCliView};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::host::Host;
use super::traits::Mechanism;
use super::{Answers, CallError, ExecOutput, HttpRequest, HttpResponse, Outcome, Step};

mod bindings {
    wasmtime::component::bindgen!({
        world: "mechanism",
        path: "wit",
    });
}

use bindings::lns::connector::types as wit;

/// The most memory one component may hold. Enough for a token exchange and its JSON, far short of anything that would matter to the machine running it.
const MAX_COMPONENT_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The most an ask's connector text — its message and every field label together — may run to. Long enough for a device-code instruction and its URL, short enough that nothing can bury lns's disclosure under it.
const MAX_CONNECTOR_TEXT_BYTES: usize = 4 * 1024;

/// The most step state a component may hold between calls. It is secret material lns keeps in memory, so it is a device code or a verifier, never a working set.
const MAX_STATE_BYTES: usize = 64 * 1024;

/// The work one declared second buys. The deadline bounds how long a component runs; fuel bounds how much it runs, so a machine under load cannot lend it less than a machine at rest.
const FUEL_PER_SECOND: u64 = 500_000_000;

struct Data {
    host: Host,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for Data {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Only what a `wasm32-wasip2` standard library needs to start; a filesystem, sockets, randomness and a wall clock are never linked, so they cannot be imported at all.
fn lend_the_standard_library<T: WasiView + WasiCliView + WasiClocksView>(
    linker: &mut Linker<T>,
) -> wasmtime::Result<()> {
    use wasmtime_wasi::p2::bindings::sync::{cli, clocks, io};

    clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    cli::exit::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::environment::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_input::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_output::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    io::error::add_to_linker::<T, HasTable>(linker, |data| data.ctx().table)?;
    io::poll::add_to_linker::<T, HasTable>(linker, |data| data.ctx().table)?;
    io::streams::add_to_linker::<T, HasTable>(linker, |data| data.ctx().table)?;
    Ok(())
}

struct HasTable;

impl wasmtime::component::HasData for HasTable {
    type Data<'a> = &'a mut ResourceTable;
}

/// One compiled engine, shared by every component this machine runs.
pub struct Runtime {
    engine: Engine,
    linker: Linker<Data>,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.consume_fuel(true);
        config.wasm_component_model(true);
        // A backtrace through a component names that component's own functions, and lns cannot read them anyway.
        config.wasm_backtrace_max_frames(None);
        let engine = Engine::new(&config)
            .map_err(|e| raised(&e).context("starting the component runtime"))?;
        let mut linker = Linker::new(&engine);
        // Linking a fixed set into a fresh linker fails only on a name collision, which would be lns's own bug; the runtime's text names it.
        lend_the_standard_library(&mut linker).map_err(|e| raised(&e))?;
        bindings::Mechanism::add_to_linker::<Data, wasmtime::component::HasSelf<Data>>(
            &mut linker,
            |data| data,
        )
        .map_err(|e| raised(&e).context("linking what lns lends a component"))?;
        Ok(Self { engine, linker })
    }

    /// Spends one second of every running component's deadline. Whoever owns the clock calls it; the runtime keeps none of its own.
    pub fn tick_once(&self) {
        self.engine.increment_epoch();
    }

    pub fn compile(&self, bytes: &[u8]) -> Result<Component> {
        if bytes.len() as u64 > lns_artifact::build::MAX_COMPONENT_BYTES {
            anyhow::bail!(
                "this connector's component is larger than the {}-byte limit",
                lns_artifact::build::MAX_COMPONENT_BYTES
            );
        }
        Ok(Component {
            component: WasmComponent::new(&self.engine, bytes).map_err(|e| {
                raised(&e).context("reading the component this method connects with")
            })?,
            engine: self.engine.clone(),
            linker: self.linker.clone(),
        })
    }
}

pub struct Component {
    engine: Engine,
    linker: Linker<Data>,
    component: WasmComponent,
}

impl Component {
    fn instance(&self, host: &Host) -> Result<(Store<Data>, bindings::Mechanism)> {
        let seconds = host.bounds().call_seconds;
        let mut store = Store::new(
            &self.engine,
            Data {
                host: host.clone(),
                // No preopens, no environment, no arguments, no sockets: what a component is not given, it cannot reach.
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(MAX_COMPONENT_MEMORY_BYTES)
                    .build(),
            },
        );
        store.limiter(|data| &mut data.limits);
        store.set_epoch_deadline(u64::from(seconds));
        store
            .set_fuel(FUEL_PER_SECOND * u64::from(seconds))
            .map_err(|e| raised(&e))?;
        let instance = bindings::Mechanism::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|e| stopped(e, seconds))?;
        Ok((store, instance))
    }
}

/// The runtime keeps its own error type, and lns reports one. Its text can still name what a component called itself, so it is scrubbed and cut to the same ceiling — cut rather than refused, because there is no connect left to fail.
fn raised(error: &wasmtime::Error) -> anyhow::Error {
    let mut said = scrubbed(&format!("{error:?}"));
    if said.len() > MAX_CONNECTOR_TEXT_BYTES {
        said.truncate(
            (0..=MAX_CONNECTOR_TEXT_BYTES)
                .rev()
                .find(|at| said.is_char_boundary(*at))
                .unwrap_or(0),
        );
    }
    anyhow!("{said}")
}

/// A component stopped at its deadline, or one that trapped, is a failed connect and never a stored connection.
fn stopped(error: wasmtime::Error, seconds: u32) -> anyhow::Error {
    match error.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::Interrupt) => {
            anyhow!("this connector's component did not finish within {seconds} seconds")
        }
        Some(wasmtime::Trap::OutOfFuel) => {
            anyhow!("this connector's component did more work than one call may do")
        }
        _ => raised(&error).context("this connector's component stopped before it answered"),
    }
}

impl Mechanism for Component {
    fn connect(&self, host: &Host, now_millis: u64) -> Result<Step> {
        let (mut store, instance) = self.instance(host)?;
        let step = instance
            .lns_connector_adapter()
            .call_connect(&mut store, now_millis)
            .map_err(|e| stopped(e, host.bounds().call_seconds))?;
        step_of(step)
    }

    fn resume(
        &self,
        host: &Host,
        state: &[u8],
        answers: &Answers,
        now_millis: u64,
    ) -> Result<Step> {
        let (mut store, instance) = self.instance(host)?;
        let step = instance
            .lns_connector_adapter()
            .call_resume(&mut store, state, &answers_of(answers), now_millis)
            .map_err(|e| stopped(e, host.bounds().call_seconds))?;
        step_of(step)
    }

    fn refresh(&self, host: &Host, values: &Answers, now_millis: u64) -> Result<Outcome> {
        let (mut store, instance) = self.instance(host)?;
        instance
            .lns_connector_adapter()
            .call_refresh(&mut store, &answers_of(values), now_millis)
            .map_err(|e| stopped(e, host.bounds().call_seconds))?
            .map(outcome_of)
            .map_err(|why| refused_in_its_own_words(&why))
    }

    fn revoke(&self, host: &Host, values: &Answers, now_millis: u64) -> Result<()> {
        let (mut store, instance) = self.instance(host)?;
        instance
            .lns_connector_adapter()
            .call_revoke(&mut store, &answers_of(values), now_millis)
            .map_err(|e| stopped(e, host.bounds().call_seconds))?
            .map_err(|why| refused_in_its_own_words(&why))
    }
}

/// A component's own account of why it could not do what it was asked, under the same rule as anything else it says.
fn refused_in_its_own_words(why: &str) -> anyhow::Error {
    match connector_text(why, why.len()) {
        Ok(said) => anyhow!("{said}"),
        Err(e) => e,
    }
}

fn answers_of(answers: &Answers) -> Vec<wit::Answer> {
    answers
        .iter()
        .map(|(name, value)| wit::Answer {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

fn step_of(step: wit::Step) -> Result<Step> {
    Ok(match step {
        wit::Step::Ask(ask) => {
            if ask.state.len() > MAX_STATE_BYTES {
                anyhow::bail!(
                    "this connector's component held more than {MAX_STATE_BYTES} bytes between calls"
                );
            }
            let spoken: usize =
                ask.message.len() + ask.fields.iter().map(|f| f.label.len()).sum::<usize>();
            Step::Ask {
                message: connector_text(&ask.message, spoken)?,
                fields: ask
                    .fields
                    .into_iter()
                    .map(|field| {
                        Ok(super::Field {
                            name: field.name,
                            label: connector_text(&field.label, spoken)?,
                            secret: field.secret,
                        })
                    })
                    .collect::<Result<_>>()?,
                state: ask.state,
            }
        }
        wit::Step::Done(outcome) => Step::Done(outcome_of(outcome)),
        wit::Step::Failed(why) => Step::Failed(connector_text(&why, why.len())?),
    })
}

/// Words a connector supplies from code nobody can read, rendered on lns's own lines and bounded: anything that could redraw a terminal could imitate the disclosure beside it, and text that forged a disclosure would forge consent (§3.2.6).
fn connector_text(words: &str, spoken: usize) -> Result<String> {
    if spoken > MAX_CONNECTOR_TEXT_BYTES {
        anyhow::bail!(
            "this connector's component spoke more than {MAX_CONNECTOR_TEXT_BYTES} bytes at once"
        );
    }
    Ok(scrubbed(words))
}

/// Nothing that could move a cursor, clear a screen, or start a line of its own survives (§3.2.6).
fn scrubbed(words: &str) -> String {
    words
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn outcome_of(outcome: wit::Outcome) -> Outcome {
    Outcome {
        values: outcome
            .values
            .into_iter()
            .map(|answer| (answer.name, answer.value))
            .collect(),
        authority: outcome.authority.into_iter().collect(),
        expires_at_millis: outcome.expires_at_millis,
    }
}

fn call_error(error: CallError) -> wit::CallError {
    match error {
        CallError::Refused(why) => wit::CallError::Refused(why),
        CallError::Failed(why) => wit::CallError::Failed(why),
    }
}

impl wit::Host for Data {}

impl bindings::lns::connector::http::Host for Data {
    fn fetch(
        &mut self,
        request: bindings::lns::connector::http::Request,
    ) -> Result<bindings::lns::connector::http::Response, wit::CallError> {
        self.host
            .fetch(&HttpRequest {
                method: request.method,
                url: request.url,
                headers: request.headers,
                body: request.body,
            })
            .map(
                |response: HttpResponse| bindings::lns::connector::http::Response {
                    status: response.status,
                    headers: response.headers,
                    body: response.body,
                },
            )
            .map_err(call_error)
    }
}

impl bindings::lns::connector::exec::Host for Data {
    fn run(
        &mut self,
        argv: Vec<String>,
    ) -> Result<bindings::lns::connector::exec::Output, wit::CallError> {
        self.host
            .run(&argv)
            .map(
                |output: ExecOutput| bindings::lns::connector::exec::Output {
                    code: output.code,
                    stdout: output.stdout,
                    stderr: output.stderr,
                },
            )
            .map_err(call_error)
    }
}

impl bindings::lns::connector::entropy::Host for Data {
    fn bytes(&mut self, count: u32) -> Vec<u8> {
        self.host.bytes(count)
    }
}

impl bindings::lns::connector::callback::Host for Data {
    fn bind(&mut self) -> Result<bindings::lns::connector::callback::Binding, wit::CallError> {
        Err(wit::CallError::Refused(
            "this build serves no loopback callback".to_string(),
        ))
    }

    fn wait(&mut self, _handle: u32) -> Result<String, wit::CallError> {
        Err(wit::CallError::Refused(
            "this build serves no loopback callback".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests;
