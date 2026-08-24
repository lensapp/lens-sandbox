//! The two verbs both namespaces answer for. `lns rm` and `lns inspect` work out which one you meant, and refuse rather than guess when both could.

use anyhow::{Result, bail};
use lns_ipc::{Request, Response};

use crate::command::{CommandSpec, subcommand};
use crate::service::client::SandboxService;

mod real;

/// Which namespace owns a word, once both have been asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Sandbox,
    Artifact,
    Both,
    Neither,
}

pub fn owner(is_a_sandbox: bool, is_an_artifact: bool) -> Owner {
    match (is_a_sandbox, is_an_artifact) {
        (true, true) => Owner::Both,
        (true, false) => Owner::Sandbox,
        (false, true) => Owner::Artifact,
        (false, false) => Owner::Neither,
    }
}

/// One word, two namespaces: `lns` never guesses, so two matches are refused by name and no match says where it looked.
pub fn resolve(verb: &str, operand: &str, owner: Owner) -> Result<Owner> {
    match owner {
        Owner::Both => bail!(
            "{operand:?} is both a sandbox and a cached artifact; say which with `lns sandbox {verb} {operand}` or `lns artifact {verb} {operand}`"
        ),
        Owner::Neither => bail!(
            "no sandbox and no cached artifact named {operand:?}; `lns sandbox ls` lists the sandboxes and `lns artifact ls` the cache"
        ),
        settled => Ok(settled),
    }
}

/// A path-shaped operand is a local document, so it never needs the service asked about it.
pub fn names_a_document(operand: Option<&str>) -> bool {
    crate::artifact::author::is_local_inspect(operand)
}

async fn ask_both(svc: &impl SandboxService, operand: &str) -> Result<Owner> {
    let is_a_sandbox = matches!(
        svc.one_shot(Request::InspectRun {
            run: operand.to_string(),
        })
        .await?,
        Response::RunInspect { .. }
    );
    let is_an_artifact = matches!(
        svc.one_shot(Request::InspectImage {
            image: operand.to_string(),
            mixins: Vec::new(),
            decisions: None,
        })
        .await?,
        Response::ImageInspected { .. }
    );
    Ok(owner(is_a_sandbox, is_an_artifact))
}

pub async fn which(svc: &impl SandboxService, verb: &str, operand: &str) -> Result<Owner> {
    resolve(verb, operand, ask_both(svc, operand).await?)
}

pub fn augment_rm(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::artifact::RmArgs>("rm").about(
        "Remove a sandbox or a cached artifact (shortcut for `lns sandbox rm` / `lns artifact rm`).",
    ))
}

pub const RM_SPEC: CommandSpec = CommandSpec {
    name: "rm",
    augment: augment_rm,
    run: real::run_rm,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub fn augment_inspect(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::artifact::InspectArgs>("inspect").about(
        "Inspect a sandbox or an artifact (shortcut for `lns sandbox inspect` / `lns artifact inspect`).",
    ))
}

pub const INSPECT_SPEC: CommandSpec = CommandSpec {
    name: "inspect",
    augment: augment_inspect,
    run: real::run_inspect,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_only_one_namespace_knows_settles_without_a_question() {
        assert_eq!(
            resolve("rm", "reviewer", Owner::Sandbox).unwrap(),
            Owner::Sandbox
        );
        assert_eq!(
            resolve("rm", "hermes:1.4.0", Owner::Artifact).unwrap(),
            Owner::Artifact
        );
    }

    #[test]
    fn a_word_both_namespaces_know_is_refused_by_name() {
        let err = resolve("inspect", "hermes", Owner::Both).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("lns sandbox inspect hermes")
                && message.contains("lns artifact inspect hermes"),
            "a refusal has to print the two commands that would settle it: {message}"
        );
    }

    #[test]
    fn a_word_neither_namespace_knows_names_where_it_looked() {
        let err = resolve("rm", "ghost", Owner::Neither).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("lns sandbox ls") && message.contains("lns artifact ls"),
            "a miss must name both namespaces as searched: {message}"
        );
    }

    #[test]
    fn the_owner_of_a_word_is_whichever_namespaces_answered_for_it() {
        assert_eq!(owner(true, true), Owner::Both);
        assert_eq!(owner(true, false), Owner::Sandbox);
        assert_eq!(owner(false, true), Owner::Artifact);
        assert_eq!(owner(false, false), Owner::Neither);
    }

    #[test]
    fn a_path_shaped_operand_is_a_document_and_a_bare_word_is_not() {
        assert!(names_a_document(None));
        assert!(names_a_document(Some(".")));
        assert!(names_a_document(Some("./lns.dev.yaml")));
        assert!(!names_a_document(Some("reviewer")));
        assert!(!names_a_document(Some("ghcr.io/team/hermes:1.4.0")));
    }
}
