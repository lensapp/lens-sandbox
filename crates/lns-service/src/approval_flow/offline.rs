use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lns_policy::{FilePolicyStore, Policy};
use tokio::sync::mpsc;

use crate::approval_flow::entries::{Entry, EntryStore, FileEntryStore};
use crate::approval_flow::notification::NoopNotifier;
use crate::approval_flow::session::{Answer, AnswerOutcome, ApprovalSession};

pub fn list(root: &Path, run_id: &str) -> Vec<Entry> {
    FileEntryStore::new(crate::cache::approvals_path(root, run_id)).list()
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
    )
    .for_run(run_id.to_string());
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
