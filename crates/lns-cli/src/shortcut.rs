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

/// The artifact namespace is the local store, so the lookup asks what the cache holds. Asking a registry instead would make every word Docker Hub can resolve an artifact, and no sandbox could be named `redis`.
fn cache_holds(
    cached: &[lns_ipc::ImageInfo],
    operand: &str,
    default_registry: Option<&str>,
) -> bool {
    let qualified = crate::config::resolve_default_registry(operand, default_registry);
    let tagged = with_default_tag(&qualified);
    cached.iter().any(|entry| {
        entry.reference == operand
            || entry.reference == qualified
            || Some(&entry.reference) == tagged.as_ref()
    })
}

fn with_default_tag(reference: &str) -> Option<String> {
    let name = reference.rsplit('/').next().unwrap_or(reference);
    (!name.contains(':') && !name.contains('@')).then(|| format!("{reference}:latest"))
}

async fn ask_both(
    svc: &impl SandboxService,
    operand: &str,
    default_registry: Option<&str>,
) -> Result<Owner> {
    let is_a_sandbox = matches!(
        svc.one_shot(Request::InspectRun {
            run: operand.to_string(),
        })
        .await?,
        Response::RunInspect { .. }
    );
    let is_an_artifact = match svc.one_shot(Request::ListImages).await? {
        Response::ImageList { images } => cache_holds(&images, operand, default_registry),
        Response::Error { message } => {
            bail!("{operand:?} cannot be arbitrated: listing the cache failed: {message}")
        }
        _ => bail!(
            "{operand:?} cannot be arbitrated: the service gave an unexpected reply to the cache listing"
        ),
    };
    Ok(owner(is_a_sandbox, is_an_artifact))
}

pub async fn which(
    svc: &impl SandboxService,
    verb: &str,
    operand: &str,
    default_registry: Option<&str>,
) -> Result<Owner> {
    resolve(
        verb,
        operand,
        ask_both(svc, operand, default_registry).await?,
    )
}

/// `-f` is `lns sandbox rm`'s flag; on an artifact operand it is refused rather than silently dropped.
pub fn rm_route(owner: Owner, force: bool) -> Result<Owner> {
    if owner == Owner::Artifact && force {
        bail!(
            "-f only applies to sandboxes: it stops a running one before removing it, and a cached artifact is never running — drop the flag or use `lns artifact rm`"
        );
    }
    Ok(owner)
}

#[derive(clap::Args)]
pub struct ShortcutRmArgs {
    #[arg(
        value_name = "REF",
        help = "Sandbox or cached artifact to remove; a running sandbox is refused unless `-f`."
    )]
    pub reference: String,

    #[arg(
        short = 'f',
        long = "force",
        default_value_t = false,
        help = "Stop a running sandbox first, then remove it. Sandboxes only."
    )]
    pub force: bool,
}

pub fn augment_rm(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<ShortcutRmArgs>("rm").about(
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
    fn force_rides_along_when_the_operand_is_a_sandbox() {
        assert_eq!(rm_route(Owner::Sandbox, true).unwrap(), Owner::Sandbox);
        assert_eq!(rm_route(Owner::Sandbox, false).unwrap(), Owner::Sandbox);
    }

    #[test]
    fn an_artifact_operand_without_force_is_untouched() {
        assert_eq!(rm_route(Owner::Artifact, false).unwrap(), Owner::Artifact);
    }

    #[test]
    fn force_on_an_artifact_operand_is_refused_by_name() {
        let err = rm_route(Owner::Artifact, true).unwrap_err().to_string();
        assert!(
            err.contains("only applies to sandboxes"),
            "the refusal says what -f is for: {err}"
        );
    }

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

    fn cached(reference: &str) -> lns_ipc::ImageInfo {
        lns_ipc::ImageInfo {
            reference: reference.to_string(),
            kind: lns_ipc::CachedKind::Sandbox,
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 0,
            layers: 0,
            pulled: "2026-01-01T00:00:00Z".into(),
            in_use_by: None,
        }
    }

    #[test]
    fn a_word_a_registry_could_resolve_is_not_an_artifact_unless_the_cache_holds_it() {
        // Every short word is a real image on Docker Hub. If the lookup asked a registry, no sandbox could be named `redis`.
        assert!(!cache_holds(&[], "redis", None));
        assert!(!cache_holds(
            &[cached("ghcr.io/team/hermes:1.4.0")],
            "redis",
            None
        ));
    }

    #[test]
    fn the_cache_answers_for_the_reference_as_typed_and_as_the_cli_qualifies_it() {
        let store = [cached("hub.lns.run/redis:latest")];
        assert!(
            cache_holds(&store, "redis", None),
            "a bare reference lives on the Lens hub, exactly where lns pull would put it"
        );
        assert!(cache_holds(&store, "hub.lns.run/redis:latest", None));
        assert!(
            !cache_holds(&store, "redis:7", None),
            "another tag is another artifact"
        );
        assert!(
            cache_holds(&[cached("ghcr.io/redis:latest")], "redis", Some("ghcr.io")),
            "run.registry decides a bare reference's home"
        );
        assert!(
            !cache_holds(&[cached("docker.io/library/redis:latest")], "redis", None),
            "Docker Hub defaults would let a Hub name shadow the user's own artifact"
        );
    }

    #[test]
    fn a_bare_reference_also_answers_for_its_untagged_cache_entry() {
        assert!(cache_holds(&[cached("hub.lns.run/redis")], "redis", None));
    }

    #[test]
    fn an_operand_no_registry_could_parse_still_matches_itself_as_typed() {
        let store = [cached("###")];
        assert!(cache_holds(&store, "###", None));
        assert!(!cache_holds(&store, "redis", None));
    }

    #[tokio::test]
    async fn a_cache_listing_error_refuses_arbitration_instead_of_guessing() {
        let svc = crate::test_service::CannedService::new(Response::Error {
            message: "cache unavailable".to_string(),
        });
        let err = which(&svc, "rm", "ghost", None).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("cache unavailable"),
            "a lookup failure fails closed and names its cause, never 'no artifact': {err:#}"
        );
    }

    #[tokio::test]
    async fn an_off_script_cache_reply_refuses_arbitration_too() {
        let svc = crate::test_service::CannedService::new(Response::Acknowledged);
        let err = which(&svc, "rm", "ghost", None).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("cannot be arbitrated"),
            "got: {err:#}"
        );
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
