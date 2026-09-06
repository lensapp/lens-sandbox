use std::path::Path;
use std::sync::Arc;

use lns_ipc::{ApprovalAnswer, ApprovalEntryKind, ApprovalInfo, Response};

use crate::approval_flow::entries::{Entry, EntryKind, RemoveOutcome, Unremovable};
use crate::approval_flow::offline;
use crate::approval_flow::session::{
    Answer, AnswerOutcome, ApprovalSession, ConnectionChoice, DecisionOutcome,
};

/// Why this entry stays, in the words that fit the kind the user named.
pub fn not_removable(kind: Unremovable) -> String {
    match kind {
        Unremovable::Destination => {
            "only a notice is removed; a destination entry is answered instead".to_string()
        }
        Unremovable::Connector => {
            "only a notice is removed; a connector entry is decided through `lns connector`"
                .to_string()
        }
    }
}

pub const DECIDED_ELSEWHERE: &str =
    "this entry is decided elsewhere: a connector through `lns connector`, and a notice not at all";

/// What this entry asks about, in the one mapping both surfaces read.
pub fn kind_of(entry: &Entry) -> ApprovalEntryKind {
    match entry.kind {
        EntryKind::Destination { .. } => ApprovalEntryKind::Destination,
        EntryKind::Connector { .. } => ApprovalEntryKind::Connector,
        EntryKind::Notice { .. } => ApprovalEntryKind::Notice,
    }
}

pub fn view(entry: &Entry) -> ApprovalInfo {
    let kind = kind_of(entry);
    let action = match &entry.kind {
        EntryKind::Destination { action, .. } => Some(action.clone()),
        EntryKind::Connector { .. } | EntryKind::Notice { .. } => None,
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

/// Every entry these runs hold, in one order. The runs arrive in whatever order the registry holds them, and a list that reorders itself between reads cannot be followed by either surface.
pub fn entries(root: &Path, runs: &[String]) -> Vec<Entry> {
    let mut held: Vec<Entry> = runs
        .iter()
        .flat_map(|run| offline::list(root, run))
        .collect();
    held.sort_by(|a, b| (&a.sandbox, a.subject(), &a.id).cmp(&(&b.sandbox, b.subject(), &b.id)));
    held
}

pub fn list(root: &Path, runs: &[String]) -> Response {
    Response::ApprovalList {
        approvals: entries(root, runs)
            .iter()
            .map(view)
            .collect::<Vec<ApprovalInfo>>(),
    }
}

/// The answer itself, for a caller that renders the outcome its own way — the service's own window does.
pub fn decide(
    root: &Path,
    runs: &[String],
    live: LiveSession,
    id: &str,
    answer: ApprovalAnswer,
) -> AnswerOutcome {
    match holder_of(root, runs, id) {
        Some((run, _)) => apply(root, &run, live, id, answer),
        None => AnswerOutcome::UnknownId,
    }
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
    render(asked, id, apply(root, &run, live, id, answer))
}

/// Removes one entry, at whichever surface asked. A notice holds no rule, so removal reaches only the list.
pub fn remove(root: &Path, runs: &[String], live: LiveSession, id: &str) -> RemoveOutcome {
    match holder_of(root, runs, id) {
        Some((run, _)) => clear(root, &run, live, id),
        None => RemoveOutcome::UnknownId,
    }
}

fn clear(root: &Path, run: &str, live: LiveSession, id: &str) -> RemoveOutcome {
    match live(run) {
        // A run this process hosts is removed through its session, which holds the store the run itself records into.
        Some(session) => session.remove_entry(id),
        None => offline::remove(root, run, id),
    }
}

/// [`remove`], rendered for the terminal.
pub fn removal(root: &Path, runs: &[String], live: LiveSession, id: &str) -> Response {
    let kept = |reason| Response::ApprovalKept {
        id: id.to_string(),
        reason,
    };
    match remove(root, runs, live, id) {
        RemoveOutcome::Removed => Response::ApprovalRemoved { id: id.to_string() },
        RemoveOutcome::UnknownId => Response::ApprovalUnknown { id: id.to_string() },
        RemoveOutcome::NotRemovable(kind) => kept(not_removable(kind)),
        RemoveOutcome::NotCleared(reason) => kept(reason),
    }
}

/// What the Approvals view can still grant on a connector row: the offer the run holds. A run that is not up, or that no longer holds the offer, has nothing for the row to answer.
pub fn offered(
    root: &Path,
    runs: &[String],
    live: LiveSession,
    id: &str,
) -> Option<lns_ipc::ConnectorView> {
    let (run, asked) = holder_of(root, runs, id)?;
    let EntryKind::Connector { name } = &asked.kind else {
        return None;
    };
    live(&run)?.offer_named(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granting {
    Granted,
    UnknownId,
    /// The run is not up, or no longer holds the offer, so this surface cannot grant it.
    NotOffered,
}

/// Grants the connector an entry names, with the card's own effect: the run's own session records it, publishes it, and releases every request the offer was holding.
pub fn grant(
    root: &Path,
    runs: &[String],
    live: LiveSession,
    id: &str,
    method: &str,
    connection: ConnectionChoice,
) -> Granting {
    let Some((run, asked)) = holder_of(root, runs, id) else {
        return Granting::UnknownId;
    };
    let EntryKind::Connector { name } = &asked.kind else {
        return Granting::NotOffered;
    };
    let Some(session) = live(&run) else {
        return Granting::NotOffered;
    };
    match session.grant_offered(name, method, connection) {
        DecisionOutcome::Resolved => Granting::Granted,
        DecisionOutcome::UnknownId => Granting::NotOffered,
    }
}

pub const NOT_OFFERED: &str =
    "this sandbox is not holding that offer; grant it with `lns connector grant`";

fn apply(
    root: &Path,
    run: &str,
    live: LiveSession,
    id: &str,
    answer: ApprovalAnswer,
) -> AnswerOutcome {
    match live(run) {
        // A live run answers through its own session, so the guest is told at once.
        Some(session) => session.answer_entry(id, answer_of(answer)),
        None => offline::answer(root, run, id, answer_of(answer)),
    }
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
    fn the_window_is_handed_the_outcome_itself_and_renders_it_its_own_way() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);

        assert_eq!(
            decide(
                home.path(),
                &runs(),
                no_live_session,
                &entry.id,
                ApprovalAnswer::AlwaysAllow
            ),
            AnswerOutcome::Recorded(EntryState::AlwaysAllowed)
        );
    }

    #[test]
    fn deciding_an_id_no_run_holds_reports_that_rather_than_writing() {
        // The window lists what it read a moment ago, so an entry removed since is the ordinary case here, not a bug.
        let home = tempfile::TempDir::new().expect("tempdir");

        assert_eq!(
            decide(
                home.path(),
                &runs(),
                no_live_session,
                "never-was",
                ApprovalAnswer::AlwaysAllow
            ),
            AnswerOutcome::UnknownId
        );
    }

    /// Grants whatever it is asked to, so the tests here pin the routing rather than the connector store.
    #[derive(Default)]
    struct WillingPort {
        granted: std::sync::Mutex<Vec<String>>,
    }

    impl crate::approval_flow::session::ConnectorPort for WillingPort {
        fn connect(
            &self,
            _: &str,
            _: &str,
            label: &str,
            _: lns_ipc::SecretValues,
        ) -> Result<Vec<String>, String> {
            self.granted
                .lock()
                .expect("granted")
                .push(format!("connect {label}"));
            Ok(Vec::new())
        }

        fn grant(
            &self,
            name: &str,
            _: &str,
            method: &str,
            connection: Option<&str>,
        ) -> Result<crate::approval_flow::protocol::GrantedPayload, String> {
            self.granted
                .lock()
                .expect("granted")
                .push(format!("grant {name} {method} {connection:?}"));
            Ok(crate::approval_flow::protocol::GrantedPayload::default())
        }

        fn decline(&self, name: &str) -> Result<(), String> {
            self.granted
                .lock()
                .expect("granted")
                .push(format!("decline {name}"));
            Ok(())
        }
    }

    fn offering(home: &Path, name: &str) -> (Arc<ApprovalSession>, Arc<WillingPort>) {
        let (sink, _nowhere) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(
            ApprovalSession::new(
                Policy::default(),
                Policy::default(),
                Arc::new(NoopNotifier),
                Arc::new(FilePolicyStore::new(crate::cache::decisions_path(
                    home, RUN,
                ))),
                sink,
                Duration::from_secs(30),
            )
            .for_run(RUN.to_string()),
        );
        session.set_entry_store(Arc::new(FileEntryStore::new(crate::cache::approvals_path(
            home, RUN,
        ))));
        let port = Arc::new(WillingPort::default());
        session.set_connector_port(port.clone());
        session.hold_for_offers(vec![lns_ipc::ConnectorView {
            name: name.to_string(),
            digest: "sha256:test".into(),
            serves: vec!["api.linear.app".into()],
            methods: Vec::new(),
            connections: Vec::new(),
        }]);
        (session, port)
    }

    #[test]
    fn a_connector_row_reads_the_offer_the_run_still_holds() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Undecided);
        let (session, _port) = offering(home.path(), "linear");
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let offer = offered(home.path(), &runs(), live_from_thread, &entry.id);

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!(
            offer.map(|offer| offer.digest),
            Some("sha256:test".to_string()),
            "the row renders the disclosure from the offer, so it must be the offer the run holds"
        );
    }

    #[test]
    fn granting_on_the_row_reaches_the_run_the_way_the_card_does() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Undecided);
        let (session, port) = offering(home.path(), "linear");
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let granted = grant(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            "token",
            ConnectionChoice::Held("work".into()),
        );

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!(granted, Granting::Granted);
        assert_eq!(
            port.granted.lock().expect("granted").as_slice(),
            [r#"grant linear token Some("work")"#],
            "the row grants through the run's own session, so the guest is told at once"
        );
        assert_eq!(
            entries(home.path(), &runs())[0].state,
            EntryState::Granted,
            "and the row reads granted afterwards"
        );
    }

    #[test]
    fn granting_with_a_new_connection_authenticates_it_first() {
        // The row offers the card's "+ new", so the value the developer typed has to reach the store before the grant that names it.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Undecided);
        let (session, port) = offering(home.path(), "linear");
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let granted = grant(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            "token",
            ConnectionChoice::New {
                label: "work".into(),
                values: lns_ipc::SecretValues(std::collections::BTreeMap::from([(
                    "token".to_string(),
                    "sk-live-real".to_string(),
                )])),
            },
        );

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!(granted, Granting::Granted);
        assert_eq!(
            port.granted.lock().expect("granted").as_slice(),
            ["connect work", r#"grant linear token Some("work")"#],
            "the connection is made first, then granted by the label it was made under"
        );
    }

    #[test]
    fn a_connector_the_run_answered_is_no_longer_the_rows_to_grant() {
        // Granting forgets the offer, so a second click on a row the list had not refreshed must not grant twice.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Undecided);
        let (session, port) = offering(home.path(), "linear");
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let first = grant(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            "token",
            ConnectionChoice::None,
        );
        let again = grant(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            "token",
            ConnectionChoice::None,
        );

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!((first, again), (Granting::Granted, Granting::NotOffered));
        assert_eq!(
            port.granted.lock().expect("granted").len(),
            1,
            "the second click reached the store exactly not at all"
        );
    }

    #[test]
    fn a_declined_connector_leaves_a_row_the_view_cannot_grant() {
        // Declining is the card's own answer; the row must read it and offer nothing, not offer a grant the run said no to.
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), connector(), EntryState::Undecided);
        let (session, port) = offering(home.path(), "linear");
        session.submit_pending(
            crate::approval_flow::protocol::RequestPending {
                id: "r1".into(),
                host: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                reason: "policy-ambiguous".into(),
                treatment: crate::approval_flow::protocol::Treatment::Inspected,
            },
            std::time::Instant::now(),
        );

        session.decline_offer("r1");

        assert_eq!(
            port.granted.lock().expect("granted").as_slice(),
            ["decline linear"]
        );
        let listed = entries(home.path(), &runs());
        let row = listed
            .iter()
            .find(|held| held.subject() == "linear")
            .expect("the connector row");
        assert_eq!(row.state, EntryState::Declined);
        assert!(
            session.offer_named("linear").is_none(),
            "a declined offer is not one the row can still grant"
        );
    }

    #[test]
    fn a_row_of_a_sandbox_that_is_not_up_offers_nothing_and_grants_nothing() {
        // Granting needs the session: it is what publishes the credentials to the guest and releases the requests the offer was holding.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Undecided);

        assert!(offered(home.path(), &runs(), no_live_session, &entry.id).is_none());
        assert_eq!(
            grant(
                home.path(),
                &runs(),
                no_live_session,
                &entry.id,
                "token",
                ConnectionChoice::None
            ),
            Granting::NotOffered
        );
    }

    #[test]
    fn a_destination_row_is_not_a_connector_to_grant() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::Undecided);
        let (session, _port) = offering(home.path(), "linear");
        LIVE.with(|live| *live.borrow_mut() = Some(session));

        let offer = offered(home.path(), &runs(), live_from_thread, &entry.id);
        let granted = grant(
            home.path(),
            &runs(),
            live_from_thread,
            &entry.id,
            "token",
            ConnectionChoice::None,
        );

        LIVE.with(|live| *live.borrow_mut() = None);
        assert!(offer.is_none());
        assert_eq!(granted, Granting::NotOffered);
    }

    #[test]
    fn granting_an_id_no_run_holds_says_so() {
        let home = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(
            grant(
                home.path(),
                &runs(),
                no_live_session,
                "never-was",
                "token",
                ConnectionChoice::None
            ),
            Granting::UnknownId
        );
    }

    #[test]
    fn a_notice_is_removed_and_the_row_goes_with_it() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), notice(), EntryState::Noted);

        assert_eq!(
            removal(home.path(), &runs(), no_live_session, &entry.id),
            Response::ApprovalRemoved {
                id: entry.id.clone()
            }
        );
        assert!(entries(home.path(), &runs()).is_empty());
    }

    #[test]
    fn a_removal_that_could_not_be_written_reaches_the_terminal_as_a_refusal() {
        // The write is a removal's only effect, so answering `ApprovalRemoved` after it failed would tell the developer the notice is gone while `ls` still lists it.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), notice(), EntryState::Noted);
        // The atomic install removes this path first and fails on anything but a missing file, so the write is refused for every uid.
        std::fs::create_dir(
            crate::cache::approvals_path(home.path(), RUN).with_extension("json.tmp"),
        )
        .expect("block the install");

        let refused = removal(home.path(), &runs(), no_live_session, &entry.id);

        let refused = serde_json::to_value(&refused).expect("responses serialize");
        assert_eq!(
            refused["type"], "ApprovalKept",
            "a failed write must not read as a removal, got {refused}"
        );
        assert_eq!(refused["id"], entry.id);
        assert!(
            refused["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("could not be written")),
            "the refusal must say what stopped it, got {refused}"
        );
        assert_eq!(
            entries(home.path(), &runs()).len(),
            1,
            "the notice is still listed, so the answer must say so"
        );
    }

    #[test]
    fn a_destination_entry_is_kept_and_told_to_answer_instead() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), destination(), EntryState::AlwaysAllowed);

        let kept = removal(home.path(), &runs(), no_live_session, &entry.id);

        assert_eq!(
            kept,
            Response::ApprovalKept {
                id: entry.id.clone(),
                reason: "only a notice is removed; a destination entry is answered instead"
                    .to_string(),
            }
        );
        assert_eq!(
            entries(home.path(), &runs()).len(),
            1,
            "the record of what the run was asked must survive the refusal"
        );
    }

    #[test]
    fn a_connector_entry_is_kept_and_told_where_it_is_decided() {
        // The refusal names the way out that fits the entry the user pointed at; a connector has no answer here to be told to use.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), connector(), EntryState::Granted);

        assert_eq!(
            removal(home.path(), &runs(), no_live_session, &entry.id),
            Response::ApprovalKept {
                id: entry.id,
                reason:
                    "only a notice is removed; a connector entry is decided through `lns connector`"
                        .to_string(),
            }
        );
    }

    #[test]
    fn removing_an_id_no_run_holds_says_so_rather_than_touching_a_file() {
        let home = tempfile::TempDir::new().expect("tempdir");
        seed(home.path(), notice(), EntryState::Noted);

        assert_eq!(
            removal(home.path(), &runs(), no_live_session, "never-was"),
            Response::ApprovalUnknown {
                id: "never-was".to_string()
            }
        );
        assert_eq!(entries(home.path(), &runs()).len(), 1);
    }

    #[test]
    fn a_live_run_is_cleared_through_its_own_session() {
        // The session holds the store's write lock, so removing behind it could write over an entry the run is recording.
        let home = tempfile::TempDir::new().expect("tempdir");
        let entry = seed(home.path(), notice(), EntryState::Noted);
        let (sink, _nowhere) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(
            ApprovalSession::new(
                Policy::default(),
                Policy::default(),
                Arc::new(NoopNotifier),
                Arc::new(FilePolicyStore::new(crate::cache::decisions_path(
                    home.path(),
                    RUN,
                ))),
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

        let outcome = remove(home.path(), &runs(), live_from_thread, &entry.id);

        LIVE.with(|live| *live.borrow_mut() = None);
        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(session.remove_entry(&entry.id) == RemoveOutcome::UnknownId);
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
