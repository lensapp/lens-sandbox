//! The schedule lns owns: a mechanism reports when its values run out, and
//! nothing else about the schedule is its to decide (`docs/sandbox-spec.md`
//! §3.2.6).

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::Result;

use super::mechanism::traits::{Mechanisms, Recorder};
use super::store::{Authority, Connection, ConnectorStore};

/// The soonest lns will refresh one connection again. A mechanism reporting less gets this, and the values still count as run out when it said they would (§3.2.6).
pub const FLOOR_MILLIS: u64 = 60_000;

/// How far ahead of an expiry lns tries, so a value is replaced before a workload meets the one that ran out.
pub const AHEAD_MILLIS: u64 = 300_000;

/// How often lns looks at what is running out. The floor bounds how often any one connection is tried; this only bounds how often lns looks.
pub const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// When each connection was last tried. Keyed by connector and label together, because a label is unique to its connector and two connectors commonly hold one named for the same method.
#[derive(Default)]
pub struct Schedule {
    tried_at: Mutex<BTreeMap<(String, Option<String>), u64>>,
    failures: Mutex<BTreeMap<(String, String), u32>>,
}

impl Schedule {
    fn failed_native(&self, connector: &str, label: &str) {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let attempts = failures
            .entry((connector.into(), label.into()))
            .or_default();
        *attempts = attempts.saturating_add(1).min(5);
    }
    fn succeeded(&self, connector: &str, label: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(connector.into(), label.into()));
    }
    fn retry_delay(&self, connector: &str, label: &str) -> u64 {
        let attempts = self
            .failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(connector.into(), label.into()))
            .copied()
            .unwrap_or_default();
        (FLOOR_MILLIS << attempts.saturating_sub(1)).min(900_000)
    }

    /// Which of one connector's connections are worth refreshing now: the ones whose reported expiry is within reach and whose floor has passed, and no others.
    pub fn due(
        &self,
        connector: &str,
        held: &BTreeMap<String, Connection>,
        now_millis: u64,
    ) -> Vec<(String, Connection)> {
        let tried_at = self.tried_at.lock().unwrap_or_else(|e| e.into_inner());
        held.iter()
            .filter(|(label, connection)| {
                if connection
                    .oauth
                    .as_ref()
                    .is_some_and(|state| state.refresh_token.is_none() || state.reconnect_required)
                {
                    return false;
                }
                let Some(expiry) = connection.expires_at_millis else {
                    // A value with no stated lifetime is one lns has no reason to believe has ended.
                    return false;
                };
                let soon_enough = expiry <= now_millis.saturating_add(AHEAD_MILLIS);
                let floor_passed = tried_at
                    .get(&(connector.to_string(), Some((*label).clone())))
                    .is_none_or(|last| {
                        now_millis.saturating_sub(*last) >= self.retry_delay(connector, label)
                    });
                soon_enough && floor_passed
            })
            .map(|(label, connection)| (label.clone(), connection.clone()))
            .collect()
    }

    /// Whether this was tried recently enough that the floor has not passed.
    fn was_tried_within_the_floor(&self, connector: &str, of: &Of<'_>, now_millis: u64) -> bool {
        self.tried_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(connector.to_string(), of.owned()))
            .is_some_and(|last| now_millis.saturating_sub(*last) < FLOOR_MILLIS)
    }

    fn tried(&self, connector: &str, of: Of<'_>, now_millis: u64) {
        self.tried_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((connector.to_string(), of.owned()), now_millis);
    }
}

/// Keeps the schedule for as long as the runtime lives. A pass is synchronous and may spend a whole call deadline, so `off_the_runtime_thread` steps it off a worker; whatever ran it on the thread that spends component deadlines would stop the clock those deadlines are measured in.
pub fn keep(
    handle: &tokio::runtime::Handle,
    mechanisms: std::sync::Arc<dyn Mechanisms>,
    recorder: std::sync::Arc<dyn Recorder>,
) {
    let schedule = Schedule::default();
    handle.spawn(async move {
        loop {
            tokio::time::sleep(LOOK_EVERY).await;
            super::mechanism::real::off_the_runtime_thread(|| {
                pass(
                    mechanisms.as_ref(),
                    recorder.as_ref(),
                    &schedule,
                    super::mechanism::real::now_millis(),
                );
            });
        }
    });
}

/// One pass over every installed connector. A machine that cannot read its own connector state renews nothing rather than stopping the loop that keeps looking.
pub fn pass(
    mechanisms: &dyn Mechanisms,
    recorder: &dyn Recorder,
    schedule: &Schedule,
    now_millis: u64,
) {
    if let Err(e) = super::real::with_every_connector(|store, name| {
        once(store, mechanisms, recorder, schedule, name, now_millis)?;
        Ok(())
    }) {
        let why = format!("{e:#}");
        crate::log::warn!("could not look at what is running out: {why}");
    }
}

/// One pass over one connector: refresh what is due, and record what came back. A refresh that fails leaves the connection as it was, so the expiry it already carries still disarms it on time (§4.1).
pub fn once(
    store: &ConnectorStore<'_>,
    mechanisms: &dyn Mechanisms,
    recorder: &dyn Recorder,
    schedule: &Schedule,
    name: &str,
    now_millis: u64,
) -> Result<Vec<String>> {
    let (definition, held) = match read_it(store, name) {
        Ok(read) => read,
        // A connector lns cannot read is spent against the floor too, so an unreadable document is reported once rather than every pass for the life of the process.
        Err(_) if schedule.was_tried_within_the_floor(name, &Of::WholeConnector, now_millis) => {
            return Ok(Vec::new());
        }
        Err(e) => {
            schedule.tried(name, Of::WholeConnector, now_millis);
            return Err(e);
        }
    };
    let digest = super::handler::installed_entry(store, name)?.digest;
    let mut renewed = Vec::new();
    for (label, connection) in schedule.due(name, &held, now_millis) {
        schedule.tried(name, Of::Connection(&label), now_millis);
        match renew(
            store,
            mechanisms,
            name,
            &definition,
            &connection,
            now_millis,
        ) {
            Ok(outcome) => {
                schedule.succeeded(name, &label);
                // Recorded before the write: a renewal that already ran at the provider happened whether or not this machine could store what came back.
                recorder.renewed(name, &label, false);
                let Some(recorded) =
                    store.record_if_current(name, &label, &digest, Some(&connection), outcome)?
                else {
                    continue;
                };
                if !recorded.invalidated.is_empty() {
                    // Those grants are gone the moment they are dropped, so this is recorded whether or not the values behind them stored.
                    let did = what_it_did(&label, recorded.invalidated.len());
                    recorder.renewed(name, &did, false);
                }
                match recorded.stored {
                    Ok(()) => renewed.push(label),
                    // One connection lns could not store must not take the connector's others with it.
                    Err(e) => {
                        crate::log::warn!(
                            "could not store the renewal of {name} as {label}: {e:#}"
                        );
                    }
                }
            }
            // A provider outage must not burn the grant: the connection stands, and its own expiry still disarms it.
            Err(e) => {
                if connection.oauth.is_some() {
                    schedule.failed_native(name, &label);
                }
                if matches!(
                    e.downcast_ref::<super::mechanism::oauth::token::TokenError>(),
                    Some(super::mechanism::oauth::token::TokenError::Reconnect)
                ) {
                    let mut disarmed = connection.clone();
                    if let Some(state) = &mut disarmed.oauth {
                        state.reconnect_required = true;
                        state.refresh_token = None;
                        disarmed.values.clear();
                        if let Some(recorded) = store.record_if_current(
                            name,
                            &label,
                            &digest,
                            Some(&connection),
                            disarmed,
                        )? {
                            recorded.stored?;
                        }
                    }
                }
                crate::log::warn!("could not renew {name} as {label}: {e:#}");
                recorder.renewed(name, &label, true);
            }
        }
    }
    Ok(renewed)
}

/// What a renewal that dropped grants is written down as: they are durable state a user must act on, and nobody was watching when it happened.
fn what_it_did(label: &str, invalidated: usize) -> String {
    format!("{label} ({invalidated} run(s) must decide again)")
}

/// What a floor is spent against: one connection, or the whole connector where lns could not even read it. An enum rather than a reserved label, so no label a user types can collide with it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Of<'a> {
    WholeConnector,
    Connection(&'a str),
}

impl Of<'_> {
    fn owned(&self) -> Option<String> {
        match self {
            Self::WholeConnector => None,
            Self::Connection(label) => Some((*label).to_string()),
        }
    }
}

/// The document and the connections one connector holds.
fn read_it(
    store: &ConnectorStore<'_>,
    name: &str,
) -> Result<(
    lns_artifact::connector::ConnectorDefinition,
    BTreeMap<String, Connection>,
)> {
    let entry = super::handler::installed_entry(store, name)?;
    Ok((
        lns_artifact::connector::parse(&entry.document)?,
        store.connections_of(name)?,
    ))
}

/// What the mechanism returns, as the connection then records it: the values and the expiry replace what was held, and scopes it omitted carry forward (§3.2.6).
fn renew(
    store: &ConnectorStore<'_>,
    mechanisms: &dyn Mechanisms,
    name: &str,
    definition: &lns_artifact::connector::ConnectorDefinition,
    connection: &Connection,
    now_millis: u64,
) -> Result<Connection> {
    let method = super::handler::offerable_method(definition, &connection.method)?;
    if connection.oauth.is_some()
        && method
            .auth
            .as_ref()
            .and_then(lns_artifact::connector::Auth::oauth)
            .is_none()
    {
        return Err(super::mechanism::oauth::token::TokenError::Reconnect.into());
    }
    let prepared = super::connect::prepare(store, mechanisms, name, definition, method)?;
    let outcome = prepared
        .mechanism
        .refresh_private(&prepared.host, connection, now_millis)?;
    // A renewal returns the same shape a connect does, so it keeps only what the method says it produces and is held to producing all of it — one rule in one place (§3.2.6).
    let values = super::connect::declared_values(method, &outcome);
    super::connect::refuse_an_answer_missing_what_the_method_produces(method, &values)?;
    Ok(Connection {
        oauth: outcome.oauth.clone(),
        generation: 0,

        method: connection.method.clone(),
        authority: if outcome.oauth.is_none() && outcome.authority.is_empty() {
            connection.authority.clone()
        } else {
            Authority::of(outcome.authority.clone())
        },
        values,
        expires_at_millis: outcome.expires_at_millis,
    })
}

#[cfg(test)]
mod tests;
