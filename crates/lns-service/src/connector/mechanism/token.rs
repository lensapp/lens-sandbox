//! `kind: token`, behind the same port a component sits behind (§3.2.6).
//!
//! It reaches nothing and runs nothing: the user holds the value already, so the
//! whole mechanism is one ask and what came back.

use anyhow::{Result, bail};

use super::host::Host;
use super::traits::Mechanism;
use super::{Answers, Field, Outcome, Step};

pub struct Token {
    outputs: Vec<String>,
    label: String,
}

impl Token {
    pub fn new(outputs: Vec<String>, label: &str) -> Self {
        Self {
            outputs,
            label: label.to_string(),
        }
    }

    /// Where the auth produces one value the author's word for it names that value; where it produces several, each output names itself.
    fn field(&self, name: &str) -> Field {
        Field {
            name: name.to_string(),
            label: match self.outputs.as_slice() {
                [_] => self.label.clone(),
                _ => name.to_string(),
            },
            secret: true,
        }
    }
}

impl Mechanism for Token {
    fn connect(&self, _host: &Host, _now_millis: u64) -> Result<Step> {
        Ok(Step::Ask {
            // lns implements this mechanism, so the words are lns's and there is no author to attribute.
            message: String::new(),
            fields: self.outputs.iter().map(|name| self.field(name)).collect(),
            state: Vec::new(),
        })
    }

    fn resume(
        &self,
        _host: &Host,
        _state: &[u8],
        answers: &Answers,
        _now_millis: u64,
    ) -> Result<Step> {
        for name in &self.outputs {
            if answers.get(name).is_none_or(|value| value.is_empty()) {
                return Ok(Step::Failed(format!("{name} was not given a value")));
            }
        }
        Ok(Step::Done(Outcome {
            // Only what the auth declared it produces: a caller that sent more sent something this mechanism does not answer for.
            values: self
                .outputs
                .iter()
                .filter_map(|name| Some((name.clone(), answers.get(name)?.clone())))
                .collect(),
            ..Outcome::default()
        }))
    }

    /// A value the user pasted is not one lns can fetch again, so the connection is offered again instead of renewed (§3.2.6).
    fn refresh(&self, _host: &Host, _values: &Answers, _now_millis: u64) -> Result<Outcome> {
        bail!("a token this machine was given cannot be renewed without the user")
    }

    fn revoke(&self, _host: &Host, _values: &Answers, _now_millis: u64) -> Result<()> {
        Ok(())
    }
}
