//! §6: what the service reports reaches the user as one sentence in the product's own words, and §2.4's wrong-kind operand also names the command that takes it.

use anyhow::{Error, anyhow};

const MISSING_RUN: &str = "no such run: ";
const MISSING_IMAGE: &str = "no such image: ";

pub(crate) fn failure(message: &str) -> Error {
    anyhow!("{message}")
}

pub(crate) fn sandbox_failure(verb: &str, run: &str, message: &str) -> Error {
    match missing_run_detail(message) {
        Some(detail) => unknown_sandbox(verb, run, detail),
        None => failure(message),
    }
}

pub(crate) fn unknown_sandbox(verb: &str, run: &str, detail: &str) -> Error {
    if reference_shaped(run) {
        let (command, effect) = artifact_counterpart(verb);
        return anyhow!(
            "no such sandbox: {run}; that looks like an artifact reference — `lns artifact {command}` {effect}"
        );
    }
    if detail.is_empty() {
        anyhow!("no such sandbox: {run}")
    } else {
        anyhow!("no such sandbox: {run}; {detail}")
    }
}

pub(crate) fn artifact_failure(message: &str) -> Error {
    let Some(reference) = message.strip_prefix(MISSING_IMAGE) else {
        return failure(message);
    };
    if could_have_been_a_sandbox_handle(reference) {
        anyhow!(
            "no such artifact: {reference}; a sandbox has a name or an id — `lns sandbox ls` lists them"
        )
    } else {
        anyhow!("no such artifact: {reference}")
    }
}

/// The service appends what would have helped — the stopped sandboxes a `start` could have named — and that half of its answer survives the rewording.
fn missing_run_detail(message: &str) -> Option<&str> {
    message
        .strip_prefix(MISSING_RUN)
        .map(|rest| rest.split_once("; ").map_or("", |(_, detail)| detail))
}

/// A sandbox is named as a DNS label or an id, so this punctuation can only have come from a registry coordinate.
fn reference_shaped(run: &str) -> bool {
    run.contains('/') || run.contains(':') || run.contains('@')
}

fn artifact_counterpart(verb: &str) -> (&'static str, &'static str) {
    match verb {
        "rm" => ("rm", "removes one"),
        _ => ("inspect", "reads one"),
    }
}

/// A bare word is what both namespaces answer for, and §2.4 qualifies it into a single untagged segment — a repository path or a chosen tag was always a reference.
fn could_have_been_a_sandbox_handle(reference: &str) -> bool {
    let repository = match reference.split_once('/') {
        Some((host, rest)) if crate::config::is_registry_host(host) => rest,
        Some(_) => return false,
        None => reference,
    };
    if repository.contains('/') {
        return false;
    }
    match repository.split_once(':') {
        Some((_, tag)) => tag == "latest",
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_failure_reaches_the_user_without_the_wire_talking() {
        let err = failure("the cache is locked by another writer");
        assert_eq!(
            format!("{err:#}"),
            "the cache is locked by another writer",
            "§6 wants one sentence, and `daemon error:` is plumbing the user never asked about"
        );
    }

    #[test]
    fn a_failure_that_is_not_a_miss_passes_through_both_namespaces_untouched() {
        assert_eq!(
            format!(
                "{:#}",
                sandbox_failure("stop", "reviewer", "the guest is wedged")
            ),
            "the guest is wedged"
        );
        assert_eq!(
            format!("{:#}", artifact_failure("the registry refused the digest")),
            "the registry refused the digest"
        );
    }

    #[test]
    fn a_run_verb_given_a_registry_coordinate_names_the_verb_that_takes_one() {
        let removal = sandbox_failure(
            "rm",
            "ghcr.io/team/x:1.0",
            "no such run: ghcr.io/team/x:1.0",
        );
        assert_eq!(
            format!("{removal:#}"),
            "no such sandbox: ghcr.io/team/x:1.0; that looks like an artifact reference — `lns artifact rm` removes one"
        );
        let stop = sandbox_failure("stop", "you/agent", "no such run: you/agent");
        assert!(
            format!("{stop:#}").contains("`lns artifact inspect` reads one"),
            "no artifact is stopped, so the reader is the way out: {stop:#}"
        );
        let digest = sandbox_failure("logs", "x@sha256:ab", "no such run: x@sha256:ab");
        assert!(
            format!("{digest:#}").contains("artifact reference"),
            "{digest:#}"
        );
    }

    #[test]
    fn a_plausible_sandbox_name_is_not_lectured_about_the_other_namespace() {
        let err = sandbox_failure("stop", "v1.2-agent", "no such run: v1.2-agent");
        assert_eq!(format!("{err:#}"), "no such sandbox: v1.2-agent");
    }

    #[test]
    fn what_the_service_added_to_a_miss_survives_the_rewording() {
        let err = sandbox_failure(
            "start",
            "ghost",
            "no such run: ghost; stopped runs: reviewer",
        );
        assert_eq!(
            format!("{err:#}"),
            "no such sandbox: ghost; stopped runs: reviewer",
            "the stopped list is the answer to \"what could I have started\", so it must not be dropped"
        );
    }

    #[test]
    fn a_run_the_service_answered_as_unknown_reads_the_same_as_one_it_reported() {
        assert_eq!(
            format!("{:#}", unknown_sandbox("inspect", "ghost", "")),
            "no such sandbox: ghost"
        );
    }

    #[test]
    fn a_ref_verb_given_a_bare_word_names_where_the_sandboxes_are_listed() {
        let err = artifact_failure("no such image: hub.lns.run/7:latest");
        assert_eq!(
            format!("{err:#}"),
            "no such artifact: hub.lns.run/7:latest; a sandbox has a name or an id — `lns sandbox ls` lists them"
        );
    }

    #[test]
    fn a_ref_verb_given_a_repository_path_says_only_that_nothing_is_cached() {
        assert_eq!(
            format!(
                "{:#}",
                artifact_failure("no such image: ghcr.io/team/x:1.0")
            ),
            "no such artifact: ghcr.io/team/x:1.0",
            "a path under a registry could never have been a sandbox handle"
        );
    }

    #[test]
    fn only_what_a_bare_word_qualifies_into_could_have_been_a_sandbox_handle() {
        assert!(could_have_been_a_sandbox_handle("redis"));
        assert!(could_have_been_a_sandbox_handle("hub.lns.run/redis"));
        assert!(!could_have_been_a_sandbox_handle("team/x"));
        assert!(
            !could_have_been_a_sandbox_handle("registry.example.test/absent:1"),
            "a chosen tag is something only a reference carries"
        );
        assert!(!could_have_been_a_sandbox_handle("hub.lns.run/x@sha256:ab"));
    }
}
