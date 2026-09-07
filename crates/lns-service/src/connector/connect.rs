//! Driving one connect: lns asks the mechanism what it wants, the caller
//! collects it, and the mechanism decides when it is done
//! (`docs/sandbox-spec.md` §3.2.6).

use anyhow::{Result, bail};
use lns_artifact::connector::{ConnectorDefinition, Method};

use super::mechanism::traits::Mechanisms;
use super::mechanism::{Answers, Field, Outcome, Step};
use super::session::{Session, Sessions};
use super::store::{Authority, Connection, ConnectorStore};

/// One turn of a connect, and the connector it belongs to — read from the session that was resumed, never from the shape of a handle.
#[derive(Debug, PartialEq, Eq)]
pub struct Turn {
    pub connector: String,
    pub connecting: Connecting,
}

/// What one turn of a connect produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Connecting {
    Asks {
        session: String,
        message: String,
        fields: Vec<Field>,
    },
    Connected(super::handler::Connected),
    Failed(String),
}

impl Connecting {
    /// What a caller who cannot be asked again gets: a mechanism still asking has nowhere to ask, so it is a failure rather than a wait.
    pub fn finished(self) -> Result<super::handler::Connected> {
        match self {
            Self::Connected(connected) => Ok(connected),
            Self::Failed(reason) => bail!("{reason}"),
            Self::Asks { fields, .. } => bail!(
                "this method is still asking for {}, and there is nowhere left to ask",
                fields
                    .iter()
                    .map(|field| field.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// What one connect is driven with: the machine's store, the mechanisms it runs, the sessions it opens, and the moment it is at.
pub struct Driver<'a> {
    pub store: ConnectorStore<'a>,
    pub mechanisms: &'a dyn Mechanisms,
    pub sessions: &'a dyn Sessions,
    pub now_millis: u64,
}

impl Driver<'_> {
    /// Begin a connect. What it asks for is the mechanism's decision, so the caller is told rather than reading it off the document.
    pub fn begin(&self, name: &str, method: &str, label: &str) -> Result<Connecting> {
        let (store, mechanisms, sessions, now_millis) =
            (&self.store, self.mechanisms, self.sessions, self.now_millis);
        let installed = super::handler::installed_entry(store, name)?;
        let definition = lns_artifact::connector::parse(&installed.document)?;
        let method = super::handler::offerable_method(&definition, method)?;
        if method.auth.is_none() {
            bail!(
                "method {} of {name} has no authentication, so there is nothing to connect; grant it instead",
                method.name
            );
        }
        let prepared = prepare(store, mechanisms, name, &definition, method)?;
        let step = prepared.mechanism.connect(&prepared.host, now_millis)?;
        settle(
            store,
            sessions,
            name,
            method,
            label,
            step,
            Deadlines {
                now_millis,
                expires_at_millis: now_millis + session_millis(method),
            },
            &installed.digest,
        )
    }

    /// Finish what one ask started. A handle this machine did not mint, or one whose session has run out, is not one it will resume.
    pub fn answer(&self, handle: &str, values: Answers) -> Result<Turn> {
        let (store, mechanisms, sessions, now_millis) =
            (&self.store, self.mechanisms, self.sessions, self.now_millis);
        let Some(session) = sessions.take(handle, now_millis) else {
            bail!("that connect is no longer open; run it again");
        };
        let installed = super::handler::installed_entry(store, &session.connector)?;
        if installed.digest != session.digest {
            // The state and the answers belong to the implementation the connect began against, and this is a different one.
            bail!("that connect is no longer open; run it again");
        }
        let definition = lns_artifact::connector::parse(&installed.document)?;
        let method = super::handler::offerable_method(&definition, &session.method)?;
        let prepared = prepare(store, mechanisms, &session.connector, &definition, method)?;
        // §3.2.6: an answer is dropped once the resume that consumed it returns, so a round sees its own values and no earlier round's.
        let step =
            prepared
                .mechanism
                .resume(&prepared.host, &session.state, &values, now_millis)?;
        Ok(Turn {
            connecting: settle(
                store,
                sessions,
                &session.connector,
                method,
                &session.label,
                step,
                Deadlines {
                    now_millis,
                    // Carried, never recomputed: sessionSeconds bounds the whole exchange, not each round of it (§3.2.6).
                    expires_at_millis: session.expires_at_millis,
                },
                &session.digest,
            )?,
            connector: session.connector,
        })
    }

    /// One connect, driven from values a caller collected before it began. The card works this way: it shows one form and presses once, so a mechanism that asks for exactly what it holds finishes, and one that asks for more says so rather than half-connecting.
    pub fn with_values(
        &self,
        name: &str,
        method: &str,
        label: &str,
        values: Answers,
    ) -> Result<Connecting> {
        let mut turn = self.begin(name, method, label)?;
        for _ in 0..MAX_ROUNDS_WITHOUT_A_PERSON {
            let Connecting::Asks {
                session, fields, ..
            } = &turn
            else {
                return Ok(turn);
            };
            if let Some(missing) = fields.iter().find(|f| !values.contains_key(&f.name)) {
                let refusal = format!(
                    "this method asks for {}, which was not collected before it ran",
                    missing.label
                );
                self.abandon(&turn);
                return Ok(Connecting::Failed(refusal));
            }
            // Only what this round asked for: an answer is dropped once the resume that consumed it returns (§3.2.6).
            let asked: Answers = fields
                .iter()
                .filter_map(|field| Some((field.name.clone(), values.get(&field.name)?.clone())))
                .collect();
            turn = self.answer(session, asked)?.connecting;
        }
        self.abandon(&turn);
        Ok(Connecting::Failed(
            "this method kept asking, and there is nobody here to answer it again".to_string(),
        ))
    }

    /// Drops a session nothing will ever answer, because it holds what was already typed.
    fn abandon(&self, turn: &Connecting) {
        if let Connecting::Asks { session, .. } = turn {
            self.sessions.take(session, self.now_millis);
        }
    }
}

/// The component this method connects with, where it has one. Read by position among the `code` methods, which is the order an install kept them in (§7).
fn prepare(
    store: &ConnectorStore<'_>,
    mechanisms: &dyn Mechanisms,
    name: &str,
    definition: &ConnectorDefinition,
    method: &Method,
) -> Result<super::mechanism::traits::Prepared> {
    let component = lns_artifact::connector::components(&definition.spec)
        .iter()
        .position(|(declared, _)| *declared == method.name)
        .map(|index| store.component(name, index))
        .transpose()?;
    mechanisms.for_method(name, method, component)
}

/// When this turn is, and when the exchange it belongs to runs out.
struct Deadlines {
    now_millis: u64,
    expires_at_millis: u64,
}

/// One answer from the mechanism, turned into what the caller sees and, where it finished, into the connection this machine then holds.
#[allow(clippy::too_many_arguments)]
fn settle(
    store: &ConnectorStore<'_>,
    sessions: &dyn Sessions,
    name: &str,
    method: &Method,
    label: &str,
    step: Step,
    at: Deadlines,
    digest: &str,
) -> Result<Connecting> {
    match step {
        Step::Failed(why) => Ok(Connecting::Failed(why)),
        Step::Ask {
            message,
            fields,
            state,
        } => {
            refuse_a_field_name_no_answer_could_be_keyed_by(&fields)?;
            let session = sessions.open(
                Session {
                    connector: name.to_string(),
                    digest: digest.to_string(),
                    method: method.name.clone(),
                    label: label.to_string(),
                    state,
                    expires_at_millis: at.expires_at_millis,
                },
                at.now_millis,
            );
            Ok(Connecting::Asks {
                session,
                message,
                fields,
            })
        }
        Step::Done(outcome) => {
            let invalidated = store.record_authentication(
                name,
                label,
                Connection {
                    method: method.name.clone(),
                    authority: Authority::of(outcome.authority.iter().cloned()),
                    values: declared_values(method, &outcome),
                },
            )?;
            Ok(Connecting::Connected(super::handler::Connected {
                connection: label.to_string(),
                invalidated,
            }))
        }
    }
}

/// Only what the method's `auth` says it produces. A mechanism returning more returned something its document never declared, and no credential could draw on it (§3.2.6).
fn declared_values(
    method: &Method,
    outcome: &Outcome,
) -> std::collections::BTreeMap<String, String> {
    method
        .auth
        .as_ref()
        .and_then(lns_artifact::connector::Auth::outputs)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| Some((name.clone(), outcome.values.get(&name)?.clone())))
        .collect()
}

/// A name is the key an answer is stored under, so one lns could not key by is refused before the user is asked for anything.
fn refuse_a_field_name_no_answer_could_be_keyed_by(fields: &[Field]) -> Result<()> {
    for field in fields {
        if field.name.trim().is_empty() || field.name.chars().any(char::is_control) {
            bail!(
                "this connector's component asked for a value under a name lns cannot key by: {:?}",
                field.name
            );
        }
    }
    Ok(())
}

/// How long the whole exchange may take, which the method declared and lns holds (§3.2.6).
fn session_millis(method: &Method) -> u64 {
    let seconds = method
        .auth
        .as_ref()
        .and_then(lns_artifact::connector::Auth::code)
        .and_then(Result::ok)
        .map_or(DEFAULT_SESSION_SECONDS, |code| {
            code.limits().session_seconds
        });
    u64::from(seconds) * 1000
}

/// How many rounds a caller who cannot be asked again will drive. Nobody is there to stop, so a mechanism that keeps asking for what it already has is stopped here.
const MAX_ROUNDS_WITHOUT_A_PERSON: usize = 8;

/// What a mechanism lns implements gets: the same ceiling a `code` method may declare, since the user is answering either way.
const DEFAULT_SESSION_SECONDS: u32 = 900;

#[cfg(test)]
mod tests;
