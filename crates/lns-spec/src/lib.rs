//! The `lns.run/v1` document grammar, specified in `docs/sandbox-spec.md`.
//!
//! One definition per concept, below every crate that reads or writes one, so a
//! sandbox, a connector and a mixin cannot drift apart in what they mean by it.

pub mod credential;
pub mod env_var;

pub use credential::{Credential, InjectionDef, InjectionKind};
pub use env_var::is_legal_env_var_name;
