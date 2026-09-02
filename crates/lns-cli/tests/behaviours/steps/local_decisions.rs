use cucumber::{given, then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::summary::{adopt_pinned_mixins, format_summary, resolved_size};

use crate::world::BehaviourWorld;

/// What the service answers for a run whose resolution reached a layered mixin: every egress entry of the merged document, in the order a first-match gate reads them.
fn resolution(w: &mut BehaviourWorld, sources: &[&str], entries: &[(&str, &str)]) {
    w.decisions.sources = sources.iter().map(|s| (*s).to_string()).collect();
    w.decisions.contributions = entries
        .iter()
        .map(|(key, source)| lns_ipc::SourceContribution {
            block: lns_ipc::ContributionBlock::Egress,
            key: (*key).to_string(),
            source: (*source).to_string(),
            note: None,
            displaced: Vec::new(),
        })
        .collect();
}

#[given(regex = r#"^the sandbox denies "([^"]+)" and a layered mixin allows it$"#)]
fn a_layered_mixin_allows_what_the_sandbox_denies(w: &mut BehaviourWorld, host: String) {
    resolution(
        w,
        &["team-egress.yaml"],
        &[
            (&format!("allow {host}"), "team-egress.yaml"),
            (&format!("deny {host}"), "the sandbox"),
        ],
    );
}

#[given(regex = r#"^the sandbox denies "([^"]+)" and a layered mixin allowed it during a run$"#)]
fn a_layered_mixin_allowed_it_during_a_run(w: &mut BehaviourWorld, host: String) {
    a_layered_mixin_allows_what_the_sandbox_denies(w, host.clone());
    let approved = format!("allow {host}");
    for entry in w
        .decisions
        .contributions
        .iter_mut()
        .filter(|c| c.key == approved)
    {
        entry.note = Some("approved during a run".to_string());
    }
}

#[given(regex = r#"^the sandbox denies "([^"]+)" and nothing is layered on it$"#)]
fn nothing_is_layered_on_it(w: &mut BehaviourWorld, host: String) {
    resolution(w, &[], &[(&format!("deny {host}"), "the sandbox")]);
}

#[given(regex = r#"^the sandbox denies "([^"]+)" with a note and nothing is layered on it$"#)]
fn the_sandbox_explains_what_it_denies(w: &mut BehaviourWorld, host: String) {
    nothing_is_layered_on_it(w, host);
    for entry in w.decisions.contributions.iter_mut() {
        entry.note = Some("the vendor mirrors the API here".to_string());
    }
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
    w.summary_output = format_summary(&args, resolved_size(Default::default(), &args));
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
            "a rule a layered mixin decided that the disclosure cannot attribute leaves an override nobody intended invisible; expected {needle:?} in:\n{}",
            w.summary_output
        ))
    }
}
