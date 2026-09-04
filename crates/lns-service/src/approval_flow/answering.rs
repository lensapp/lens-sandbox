use std::path::Path;
use std::sync::Arc;

use lns_ipc::{ApprovalAnswer, ApprovalEntryKind, ApprovalInfo, Response};

use crate::approval_flow::entries::{Entry, EntryKind};
use crate::approval_flow::offline;
use crate::approval_flow::session::{Answer, AnswerOutcome, ApprovalSession};

const DECIDED_ELSEWHERE: &str =
    "this entry is decided elsewhere: a connector through `lns connector`, and a notice not at all";

pub fn view(entry: &Entry) -> ApprovalInfo {
    let (kind, action) = match &entry.kind {
        EntryKind::Destination { action, .. } => {
            (ApprovalEntryKind::Destination, Some(action.clone()))
        }
        EntryKind::Connector { .. } => (ApprovalEntryKind::Connector, None),
        EntryKind::Notice { .. } => (ApprovalEntryKind::Notice, None),
    };
    ApprovalInfo {
        id: entry.id.clone(),
        sandbox: entry.sandbox.clone(),
        subject: entry.subject().to_string(),
        action,
        kind,
        answer: entry.state.label().to_string(),
        answerable: entry.is_answerable(),
    }
}

fn answer_of(answer: ApprovalAnswer) -> Answer {
    match answer {
        ApprovalAnswer::AlwaysAllow => Answer::AlwaysAllow,
        ApprovalAnswer::AlwaysDeny => Answer::AlwaysDeny,
        ApprovalAnswer::AskAgain => Answer::AskAgain,
    }
}

/// Where a live run's session is found; a run this process is not hosting has none, and is answered through its own files.
pub type LiveSession = fn(&str) -> Option<Arc<ApprovalSession>>;

pub fn list(root: &Path, runs: &[String]) -> Response {
    let mut approvals: Vec<ApprovalInfo> = runs
        .iter()
        .flat_map(|run| offline::list(root, run))
        .map(|entry| view(&entry))
        .collect();
    // The runs arrive in whatever order the registry holds them, and a list that reorders itself between invocations cannot be read or diffed.
    approvals.sort_by(|a, b| (&a.sandbox, &a.subject, &a.id).cmp(&(&b.sandbox, &b.subject, &b.id)));
    Response::ApprovalList { approvals }
}

pub fn answer(
    root: &Path,
    runs: &[String],
    live: LiveSession,
    id: &str,
    answer: ApprovalAnswer,
) -> Response {
    let Some((run, asked)) = holder_of(root, runs, id) else {
        return Response::ApprovalUnknown { id: id.to_string() };
    };
    let outcome = match live(&run) {
        // A live run answers through its own session, so the guest is told at once.
        Some(session) => session.answer_entry(id, answer_of(answer)),
        None => offline::answer(root, &run, id, answer_of(answer)),
    };
    render(asked, id, outcome)
}

fn holder_of(root: &Path, runs: &[String], id: &str) -> Option<(String, Entry)> {
    runs.iter().find_map(|run| {
        offline::list(root, run)
            .into_iter()
            .find(|entry| entry.id == id)
            .map(|entry| (run.clone(), entry))
    })
}

fn render(asked: Entry, id: &str, outcome: AnswerOutcome) -> Response {
    match outcome {
        AnswerOutcome::Recorded(state) => Response::ApprovalAnswered {
            approval: view(&Entry { state, ..asked }),
        },
        AnswerOutcome::UnknownId => Response::ApprovalUnknown { id: id.to_string() },
        AnswerOutcome::NotAnswerable => Response::ApprovalNotWritten {
            id: id.to_string(),
            reason: DECIDED_ELSEWHERE.to_string(),
        },
        AnswerOutcome::NoRuleWritten(reason) => Response::ApprovalNotWritten {
            id: id.to_string(),
            reason,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::entries::{EntryState, EntryStore, FileEntryStore};
    use crate::approval_flow::notification::NoopNotifier;
    use lns_policy::{FilePolicyStore, Policy};
    use std::time::Duration;

    const RUN: &str = "aa01";

    fn seed(root: &Path, kind: EntryKind, state: EntryState) -> Entry {
        let entry = Entry::new(Some(RUN.to_string()), kind, state);
        let path = crate::cache::approvals_path(root, RUN);
        std::fs::create_dir_all(path.parent().expect("run dir")).expect("run dir");
        FileEntryStore::new(path).record(entry.clone());
        entry
    }

    fn destination() -> EntryKind {
        EntryKind::Destination {
            destination: "api.linear.app".into(),
            action: "CONNECT api.linear.app:443".into(),
            raw: false,
        }
    }

    fn connector() -> EntryKind {
        EntryKind::Connector {
            name: "linear".into(),
        }
    }

    fn notice() -> EntryKind {
        EntryKind::Notice {
            message: "the rule could not be persisted".into(),
        }
    }

    fn no_live_session(_: &str) -> Option<Arc<ApprovalSession>> {
        None
    }

    fn runs() -> Vec<String> {
        vec![RUN.to_string()]
    }

    #[test]
    fn a_destination_lists_as_answerable_with_the_action_the_card_showed() {
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), destination(), EntryState::Undecided);

        let entry = Entry::new(Some(RUN.to_string()), destination(), EntryState::Undecided);
        assert_eq!(
            list(home.path(), &runs()),
            Response::ApprovalList {
                approvals: vec![ApprovalInfo {
                    id: entry.id,
                    sandbox: Some(RUN.to_string()),
                    subject: "api.linear.app".to_string(),
                    action: Some("CONNECT api.linear.app:443".to_string()),
                    kind: ApprovalEntryKind::Destination,
                    answer: "undecided".to_string(),
                    answerable: true,
                }]
            }
        );
    }

    #[test]
    fn a_connector_and_a_notice_list_as_answered_elsewhere() {
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), connector(), EntryState::Granted);
        seed(home.path(), notice(), EntryState::Noted);

        let listed = list(home.path(), &runs());
        let rendered = serde_json::to_string(&listed).expect("responses serialize");
        assert_eq!(
            listed,
            Response::ApprovalList {
                approvals: vec![
                    ApprovalInfo {
                        id: Entry::new(Some(RUN.to_string()), connector(), EntryState::Granted).id,
                        sandbox: Some(RUN.to_string()),
                        subject: "linear".to_string(),
                        action: None,
                        kind: ApprovalEntryKind::Connector,
                        answer: "granted".to_string(),
                        answerable: false,
                    },
                    ApprovalInfo {
                        id: Entry::new(Some(RUN.to_string()), notice(), EntryState::Noted).id,
                        sandbox: Some(RUN.to_string()),
                        subject: "the rule could not be persisted".to_string(),
                        action: None,
                        kind: ApprovalEntryKind::Notice,
                        answer: "notice".to_string(),
                        answerable: false,
                    },
                ]
            },
            "got {rendered}"
        );
    }

    #[test]
    fn the_list_reads_the_same_way_every_time() {
        // The registry hands runs over in hash order, so without a sort the rows move between invocations.
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), destination(), EntryState::Undecided);
        seed(home.path(), connector(), EntryState::Granted);
        seed(home.path(), notice(), EntryState::Noted);

        let subjects = |response: &Response| {
            serde_json::to_value(response).expect("responses serialize")["approvals"]
                .as_array()
                .expect("rows")
                .iter()
                .map(|row| row["subject"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        };

        assert_eq!(
            subjects(&list(home.path(), &runs())),
            vec![
                "api.linear.app".to_string(),
                "linear".to_string(),
                "the rule could not be persisted".to_string(),
            ]
        );
    }

    #[test]
    fn a_run_nothing_was_asked_about_lists_nothing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            list(home.path(), &runs()),
            Response::ApprovalList {
                approvals: Vec::new()
            }
        );
    }

    #[test]
    fn a_stopped_run_takes_the_answer_and_the_row_says_so() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);

        let answered = answer(
            home.path(),
            &runs(),
            no_live_session,
            &entry.id,
            ApprovalAnswer::AlwaysAllow,
        );

        assert_eq!(answer_words(&answered).as_deref(), Some("always allow"));
    }

    #[test]
    fn a_live_run_is_answered_through_its_own_session() {
        // The session is what tells the guest, so a run this process hosts must not be answered behind its back.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);
        let decisions = crate::cache::decisions_path(home.path(), RUN);
        let (sink, _nowhere) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(
            ApprovalSession::new(
                Policy::default(),
                Policy::default(),
                Arc::new(NoopNotifier),
                Arc::new(FilePolicyStore::new(decisions.clone())),
                sink,
                Duration::from_secs(30),
            )
            .for_run(RUN.to_string()),
        );
        session.set_entry_store(Arc::new(FileEntryStore::new(crate::cache::approvals_path(
            home.path(),
            RUN,
        ))));
        LIVE.with(|live| *live.borrow_mut() = Some(session.clone()));

        let answered = answer(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            ApprovalAnswer::AlwaysDeny,
        );

        assert_eq!(answer_words(&answered).as_deref(), Some("always deny"));
        assert!(
            session
                .current_policy()
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.match_pattern == "api.linear.app"),
            "the live session's own policy must carry the rule, not just the file"
        );
        LIVE.with(|live| *live.borrow_mut() = None);
    }

    thread_local! {
        static LIVE: std::cell::RefCell<Option<Arc<ApprovalSession>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn live_from_thread(_: &str) -> Option<Arc<ApprovalSession>> {
        LIVE.with(|live| live.borrow().clone())
    }

    /// The answer an entry ended up with, read off the response the way a caller reads it.
    fn answer_words(response: &Response) -> Option<String> {
        serde_json::to_value(response).expect("responses serialize")["approval"]["answer"]
            .as_str()
            .map(str::to_string)
    }

    #[test]
    fn an_entry_the_live_session_does_not_know_is_reported_rather_than_answered() {
        // The file and the session can disagree — a hand-edited approvals file is the ordinary way — and a miss there must not read as an answer.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);
        let elsewhere = tempfile::TempDir::new().expect("tempdir");
        let (sink, _nowhere) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            Arc::new(NoopNotifier),
            Arc::new(FilePolicyStore::new(
                elsewhere.path().join("decisions.yaml"),
            )),
            sink,
            Duration::from_secs(30),
        ));
        session.set_entry_store(Arc::new(FileEntryStore::new(
            elsewhere.path().join("approvals.json"),
        )));
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let answered = answer(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            ApprovalAnswer::AlwaysAllow,
        );

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!(answered, Response::ApprovalUnknown { id: entry.id });
    }

    #[test]
    fn an_id_no_run_holds_is_unknown() {
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), destination(), EntryState::Undecided);

        assert_eq!(
            answer(
                home.path(),
                &runs(),
                no_live_session,
                "never-was",
                ApprovalAnswer::AlwaysAllow
            ),
            Response::ApprovalUnknown {
                id: "never-was".to_string()
            }
        );
    }

    #[test]
    fn a_connector_entry_is_not_answered_here_and_says_where_it_is() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(
            home.path(),
            EntryKind::Connector {
                name: "linear".into(),
            },
            EntryState::Granted,
        );

        let refused = answer(
            home.path(),
            &runs(),
            no_live_session,
            &entry.id,
            ApprovalAnswer::AlwaysDeny,
        );

        assert_eq!(
            refused,
            Response::ApprovalNotWritten {
                id: entry.id,
                reason: DECIDED_ELSEWHERE.to_string(),
            }
        );
    }

    #[test]
    fn asking_again_about_an_entry_that_wrote_no_rule_says_so() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);

        let refused = answer(
            home.path(),
            &runs(),
            no_live_session,
            &entry.id,
            ApprovalAnswer::AskAgain,
        );

        assert_eq!(
            refused,
            Response::ApprovalNotWritten {
                id: entry.id,
                reason: crate::approval_flow::session::NO_RULE_OF_ITS_OWN.to_string(),
            }
        );
    }
}
