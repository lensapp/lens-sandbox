use std::path::Path;

use cucumber::{given, then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::summary::{PolicySource, adopt_pinned_mixins, format_summary, resolved_size};
use lns_policy::Policy;

use crate::world::BehaviourWorld;

/// What the service answers for a directory whose decisions the resolution reached: every egress entry of the merged document, in the order a first-match gate reads them.
fn resolution(w: &mut BehaviourWorld, sources: &[&str], entries: &[(&str, &str)]) {
    w.decisions.sources = sources.iter().map(|s| (*s).to_string()).collect();
    w.decisions.contributions = entries
        .iter()
        .map(|(key, source)| lns_ipc::SourceContribution {
            block: lns_ipc::ContributionBlock::Egress,
            key: (*key).to_string(),
            source: (*source).to_string(),
            displaced: Vec::new(),
        })
        .collect();
}

#[given(regex = r#"^the sandbox denies "([^"]+)" and this directory allows it$"#)]
fn the_directory_allows_what_the_sandbox_denies(w: &mut BehaviourWorld, host: String) {
    resolution(
        w,
        &["lns-local-mixin.yaml"],
        &[
            (&format!("allow {host}"), "lns-local-mixin.yaml"),
            (&format!("deny {host}"), "the sandbox"),
        ],
    );
}

#[given(regex = r#"^the sandbox denies "([^"]+)" and this directory decided nothing$"#)]
fn the_directory_decided_nothing(w: &mut BehaviourWorld, host: String) {
    resolution(w, &[], &[(&format!("deny {host}"), "the sandbox")]);
}

#[when("the run summary is composed before boot")]
fn compose_the_summary(w: &mut BehaviourWorld) {
    let mut args: RunArgs = parse_args(&[
        "lns".to_string(),
        "run".to_string(),
        "ghcr.io/team/hermes:1".to_string(),
    ])
    .expect("argv must parse against the CLI grammar");
    let sources = std::mem::take(&mut w.decisions.sources);
    let contributions = std::mem::take(&mut w.decisions.contributions);
    adopt_pinned_mixins(&mut args, &sources, &[], &contributions);
    w.summary_output = format_summary(
        &args,
        resolved_size(Default::default(), &args),
        &Policy::default(),
        Path::new("./lns-local-mixin.yaml"),
        &PolicySource::FoundInCwd,
    );
}

#[then(regex = r#"^the run summary lists "([^"]+)"$"#)]
fn summary_lists(w: &mut BehaviourWorld, line: String) -> Result<(), String> {
    if w.summary_output.contains(&line) {
        Ok(())
    } else {
        Err(format!("expected {line:?} in:\n{}", w.summary_output))
    }
}

#[then(regex = r#"^the run summary attributes "([^"]+)" to "([^"]+)"$"#)]
fn summary_attributes(w: &mut BehaviourWorld, entry: String, source: String) -> Result<(), String> {
    let needle = format!("{entry}  [from {source}]");
    if w.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "a rule this directory decided that the disclosure cannot attribute leaves an override nobody intended invisible; expected {needle:?} in:\n{}",
            w.summary_output
        ))
    }
}
