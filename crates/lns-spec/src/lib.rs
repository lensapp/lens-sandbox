//! The `lns.run/v1` document grammar, specified in `docs/sandbox-spec.md`.
//!
//! One definition per concept, below every crate that reads or writes one, so a
//! sandbox, a connector and a mixin cannot drift apart in what they mean by it.

pub mod credential;

pub use credential::{Credential, InjectionDef, InjectionKind, is_self_identifying};
