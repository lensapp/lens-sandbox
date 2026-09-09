use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lns_policy::{FilePolicyStore, Policy};
use tokio::sync::mpsc;

use crate::approval_flow::entries::{Entry, EntryStore, FileEntryStore, RemoveOutcome};
use crate::approval_flow::notification::NoopNotifier;
use crate::approval_flow::session::{Answer, AnswerOutcome, ApprovalSession};

pub fn list(root: &Path, run_id: &str) -> Vec<Entry> {
    FileEntryStore::new(crate::cache::approvals_path(root, run_id)).list()
}

/// Removes an entry of a run this process is not hosting. Nothing is published: a notice held no rule, so no guest has anything to be told.
pub fn remove(root: &Path, run_id: &str, id: &str) -> RemoveOutcome {
    crate::approval_flow::entries::remove_from(
        &FileEntryStore::new(crate::cache::approvals_path(root, run_id)),
        id,
    )
}

/// The sandbox the answered entry is stamped with, which is what a notice raised beside it must carry too.
fn sandbox_stamping(root: &Path, run_id: &str, id: &str) -> Option<String> {
    list(root, run_id)
        .into_iter()
        .find(|held| held.id == id)
        .and_then(|held| held.sandbox)
}

/// Answers an entry of a run this process is not hosting: the same session logic over the run's own files, publishing to nobody because no guest is listening.
pub fn answer(root: &Path, run_id: &str, id: &str, answer: Answer) -> AnswerOutcome {
    let decisions = crate::cache::decisions_path(root, run_id);
    // A file we cannot read is not an empty one: answering over it would write the run's whole decisions away.
    let held = match Policy::load_or_default(&decisions) {
        Ok(held) => held,
        Err(e) => {
            return AnswerOutcome::NoRuleWritten(format!(
                "this run's decisions could not be read, so nothing was written: {e}"
            ));
        }
    };
    let (sink, _nowhere) = mpsc::unbounded_channel();
    let session = ApprovalSession::new(
        held.clone(),
        held,
        Arc::new(NoopNotifier),
        Arc::new(FilePolicyStore::new(decisions)),
        sink,
        Duration::from_secs(0),
    );
    let session = match sandbox_stamping(root, run_id, id) {
        Some(sandbox) => session.for_run(sandbox),
        None => session,
    };
    session.set_entry_store(Arc::new(FileEntryStore::new(crate::cache::approvals_path(
        root, run_id,
    ))));
    session.answer_entry(id, answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::entries::{EntryKind, EntryState};
    use lns_policy::Verdict;

    fn seed(root: &Path, run: &str, state: EntryState) -> Entry {
        let entry = Entry::new(
            Some(run.to_string()),
            EntryKind::Destination {
                destination: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                raw: false,
            },
            state,
        );
        let path = crate::cache::approvals_path(root, run);
        std::fs::create_dir_all(path.parent().expect("run dir")).expect("run dir");
        FileEntryStore::new(path).record(entry.clone());
        entry
    }

    #[test]
    fn a_stopped_run_takes_an_answer_and_keeps_it_in_its_own_files() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), "aa01", EntryState::Undecided);

        let outcome = answer(home.path(), "aa01", &entry.id, Answer::AlwaysAllow);

        assert_eq!(outcome, AnswerOutcome::Recorded(EntryState::AlwaysAllowed));
        let decided = Policy::load_or_default(&crate::cache::decisions_path(home.path(), "aa01"))
            .expect("read the run's decisions");
        let rule = &decided.network.egress.http[0];
        assert_eq!(rule.match_pattern, "api.linear.app");
        assert_eq!(rule.verdict, Verdict::Allow);
        assert_eq!(
            list(home.path(), "aa01")[0].state,
            EntryState::AlwaysAllowed
        );
    }

    #[test]
    fn asking_again_about_a_stopped_run_takes_its_rule_back() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), "aa01", EntryState::Undecided);
        answer(home.path(), "aa01", &entry.id, Answer::AlwaysAllow);

        let outcome = answer(home.path(), "aa01", &entry.id, Answer::AskAgain);

        assert_eq!(outcome, AnswerOutcome::Recorded(EntryState::Undecided));
        let decided = Policy::load_or_default(&crate::cache::decisions_path(home.path(), "aa01"))
            .expect("read the run's decisions");
        assert!(decided.network.egress.http.is_empty());
    }

    #[test]
    fn a_decisions_file_that_cannot_be_read_is_left_exactly_as_it_is() {
        // Treating an unparseable file as empty would rewrite it with one rule and take every rule the developer wrote with it.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), "aa01", EntryState::Undecided);
        let decisions = crate::cache::decisions_path(home.path(), "aa01");
        let broken = "apiVersion: lns.run/v1\nkind: mixin\nname: decisions\nspec:\n  egress:\n    http:\n      - match: api.github.com\n        verdict: allow\n  : not yaml\n";
        std::fs::write(&decisions, broken).expect("write the broken file");

        let outcome = answer(home.path(), "aa01", &entry.id, Answer::AlwaysAllow);

        assert!(
            matches!(outcome, AnswerOutcome::NoRuleWritten(_)),
            "an unreadable file refuses the answer, got {outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&decisions).expect("read back"),
            broken,
            "the run's decisions must be untouched"
        );
    }

    #[test]
    fn clearing_an_answered_entry_leaves_the_rule_it_wrote() {
        // The list is what the run was asked; clearing a line of it clears the question, never the answer's effect (cli-spec §3.7).
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), "aa01", EntryState::Undecided);
        answer(home.path(), "aa01", &entry.id, Answer::AlwaysAllow);
        let decisions = crate::cache::decisions_path(home.path(), "aa01");
        let before = std::fs::read_to_string(&decisions).expect("the rule was written");

        let outcome = remove(home.path(), "aa01", &entry.id);

        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(list(home.path(), "aa01").is_empty(), "the line is gone");
        assert_eq!(
            std::fs::read_to_string(&decisions).expect("read back"),
            before,
            "and the run still decides that destination exactly as it did"
        );
    }

    #[test]
    fn a_notice_an_offline_answer_raises_is_filed_under_the_sandbox_the_list_names() {
        // Every other entry of the run carries the name the cards spoke for, and the id is a digest of that name: stamping a notice with the run id files it under a sandbox of its own.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = Entry::new(
            Some("reviewer".to_string()),
            EntryKind::Destination {
                destination: "db.internal:5432".into(),
                action: "CONNECT db.internal:5432".into(),
                raw: true,
            },
            EntryState::Undecided,
        );
        let path = crate::cache::approvals_path(home.path(), "aa01");
        std::fs::create_dir_all(path.parent().expect("run dir")).expect("run dir");
        FileEntryStore::new(path).record(entry.clone());
        let mut held = Policy::default();
        held.add_rule(lns_policy::RouteRule::allow_host("db.internal"));
        held.save_atomic(&crate::cache::decisions_path(home.path(), "aa01"))
            .expect("seed the run's decisions");

        answer(home.path(), "aa01", &entry.id, Answer::AlwaysAllow);

        let noticed: Vec<Option<String>> = list(home.path(), "aa01")
            .into_iter()
            .filter(|held| matches!(held.kind, EntryKind::Notice { .. }))
            .map(|held| held.sandbox)
            .collect();
        assert_eq!(
            noticed,
            vec![Some("reviewer".to_string())],
            "the notice must sit under the same sandbox as the question that raised it"
        );
    }

    #[test]
    fn an_entry_no_run_holds_is_not_answered() {
        let home = tempfile::TempDir::new().expect("tempdir");

        assert_eq!(
            answer(home.path(), "aa01", "never-was", Answer::AlwaysAllow),
            AnswerOutcome::UnknownId
        );
    }

    #[test]
    fn a_run_that_was_asked_nothing_lists_nothing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        assert!(list(home.path(), "aa01").is_empty());
    }
}
