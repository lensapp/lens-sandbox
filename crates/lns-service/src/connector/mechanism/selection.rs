//! Which mechanism a method connects with, and the bounds lns holds around it.
//!
//! Separate from the leaf adapters it is wired to, because this is a decision:
//! a method carrying code runs its own implementation, and one that does not
//! runs the mechanism lns implements.

use anyhow::{Result, bail};
use lns_artifact::connector::Method;

use super::Bounds;
use super::real::RealMechanisms;
use super::traits::{Mechanisms, Prepared};

impl Mechanisms for RealMechanisms {
    fn for_method(
        &self,
        connector: &str,
        method: &Method,
        component: Option<Vec<u8>>,
    ) -> Result<Prepared> {
        let Some(auth) = method.auth.as_ref() else {
            bail!("method {} has no authentication to run", method.name);
        };
        let Some(code) = auth.code() else {
            let outputs = auth.outputs().unwrap_or_else(|| vec![auth.kind.clone()]);
            return Ok(Prepared {
                mechanism: Box::new(super::token::Token::new(outputs, auth.label())),
                host: self.host(connector, Bounds::default()),
            });
        };
        let code = code?;
        let Some(bytes) = component else {
            bail!(
                "method {} names a component this machine did not keep; reinstall the connector",
                method.name
            );
        };
        Ok(Prepared {
            mechanism: Box::new(self.runtime.compile(&bytes)?),
            host: self.host(connector, Bounds::of(&code)),
        })
    }
}

#[cfg(test)]
mod tests;
