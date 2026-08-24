use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};
use lns_policy::host_path_decisions::{
    HostPathDecision, HostPathDecisionFile, HostPathDecisionStore, decision_key, repository_of,
};

use crate::run::summary::FilesetSummary;

/// The host paths a pulled sandbox may not read, so the run mounts what the developer granted and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostPathGrant {
    pub denied: Vec<String>,
}

/// Where the document came from. A document in the developer's own directory is their own consent, so only a pulled one carries a reference to decide against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentOrigin {
    OwnDirectory,
    Pulled { reference: String },
}

/// A published artifact names a host file; whether it is read is this machine's decision, recorded per repository so a version bump does not ask again.
pub fn decide_host_paths(
    origin: &DocumentOrigin,
    filesets: &[FilesetSummary],
    store: &HostPathDecisionStore,
    assume_yes: bool,
    interactive: bool,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<HostPathGrant> {
    let declared: Vec<Declared<'_>> = filesets
        .iter()
        .filter_map(|fileset| {
            Some(Declared {
                host_path: fileset.host_path.as_deref()?,
                decider: decider(origin, fileset)?,
                optional: fileset.optional,
                guest_path: &fileset.guest_path,
            })
        })
        .collect();
    if declared.is_empty() {
        return Ok(HostPathGrant::default());
    }
    let mut recorded = store
        .load()
        .context("reading this machine's host file decisions")?;
    let mut settled = Vec::with_capacity(declared.len());
    for entry in declared {
        let key = decision_key(entry.decider, entry.host_path);
        let decision = match recorded.get(&key) {
            Some(decision) => *decision,
            // `--yes` answers a question this machine has not been asked; it does not overrule one it already answered.
            None if assume_yes => HostPathDecision::Allow,
            None => {
                let asked = ask(&entry, interactive, input, output)?;
                recorded.insert(key, asked);
                save(store, &recorded)?;
                asked
            }
        };
        settled.push((entry, decision));
    }
    let mut grant = HostPathGrant::default();
    for (entry, decision) in settled {
        apply(decision, &entry, &mut grant)?;
    }
    Ok(grant)
}

/// One host file the run must be granted, and the artifact whose declaration asks for it — a mixin is that artifact even when a local document layered it in.
struct Declared<'a> {
    host_path: &'a str,
    decider: &'a str,
    optional: bool,
    guest_path: &'a str,
}

/// Which artifact's declaration this entry rides on, or `None` when the developer's own document declared it and no decision is owed.
fn decider<'a>(origin: &'a DocumentOrigin, fileset: &'a FilesetSummary) -> Option<&'a str> {
    if let Some(mixin) = fileset.declared_by.as_deref() {
        return Some(mixin);
    }
    match origin {
        DocumentOrigin::OwnDirectory => None,
        DocumentOrigin::Pulled { reference } => Some(reference),
    }
}

fn apply(
    decision: HostPathDecision,
    entry: &Declared<'_>,
    grant: &mut HostPathGrant,
) -> Result<()> {
    if decision == HostPathDecision::Allow {
        return Ok(());
    }
    let host_path = entry.host_path;
    if !entry.optional {
        bail!(
            "declined: {host_path} is required by this sandbox, so there is nothing to run without it — allow it, or edit {}",
            lns_ipc::host_path_decisions_path().display()
        );
    }
    grant.denied.push(host_path.to_string());
    Ok(())
}

fn ask(
    entry: &Declared<'_>,
    interactive: bool,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<HostPathDecision> {
    let host_path = entry.host_path;
    let reference = entry.decider;
    let repository = repository_of(reference);
    writeln!(
        output,
        "{repository} wants to read {host_path} from this machine and mount a copy at {}.",
        entry.guest_path
    )?;
    if !interactive {
        bail!(
            "{reference} reads {host_path} from this machine and there is no terminal to decide — run interactively, or pass --yes to accept it"
        );
    }
    write!(output, "Let it read {host_path}? [y/N]: ")?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(if answer == "y" || answer == "yes" {
        HostPathDecision::Allow
    } else {
        HostPathDecision::Deny
    })
}

/// A failed write must not pass for a recorded answer: the developer is asked again next run rather than told a decision was kept.
fn save(store: &HostPathDecisionStore, recorded: &HostPathDecisionFile) -> Result<()> {
    store
        .save(recorded)
        .context("recording this machine's host file decision")
}
