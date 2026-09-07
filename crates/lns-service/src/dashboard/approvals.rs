use std::collections::BTreeSet;

use lns_ipc::ApprovalAnswer;

use crate::approval_flow::answering::Granting;
use crate::approval_flow::entries::{Entry, EntryKind, EntryState, RemoveOutcome};
use crate::approval_flow::session::AnswerOutcome;
use crate::dashboard::Sandbox;

/// Every answer a row can carry, in the order the chooser offers them. Read off the states themselves, so the chooser cannot offer one no row has. An empty choice is every answer, as the audit view's kinds are.
pub fn answers() -> Vec<&'static str> {
    EntryState::ALL.iter().map(EntryState::label).collect()
}

/// How many questions in this sandbox's list still wait for an answer — the count beside the view, and the reason a closed card is recoverable at all.
///
/// The chosen answers are deliberately not applied: the chooser lives inside the view, and a count that read `0` because of a filter nobody can see from the sidebar would say the opposite of the truth.
pub fn waiting(entries: &[Entry], selected: Option<&str>, sandboxes: &[Sandbox]) -> usize {
    listing(entries, selected, sandboxes, &BTreeSet::new())
        .rows
        .iter()
        .filter(|i| waits(&entries[**i]))
        .count()
}

/// What the view lists.
pub struct Listing {
    pub rows: Vec<usize>,
}

/// The rows the view shows: what one sandbox was asked, carrying one of the chosen answers, or everything when neither is chosen.
///
/// An entry is stamped with the run's name, and the sidebar selects a run by id, so the selection is resolved through the sandbox list before it can match.
pub fn listing(
    entries: &[Entry],
    selected: Option<&str>,
    sandboxes: &[Sandbox],
    answers: &BTreeSet<String>,
) -> Listing {
    let asked = selected.map(|id| named(id, sandboxes));
    let rows: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| match asked {
            Some(run) => entry.sandbox.as_deref() == Some(run),
            None => true,
        })
        .filter(|(_, entry)| answers.is_empty() || answers.contains(entry.state.label()))
        .map(|(i, _)| i)
        .collect();
    Listing { rows }
}

/// How loudly a row's answer reads. The answer is the thing the developer came back for, so it is not a muted footnote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Allowed,
    Denied,
    Quiet,
}

pub fn tone(entry: &Entry) -> Tone {
    match entry.state {
        EntryState::Undecided => Tone::Waiting,
        EntryState::AlwaysAllowed | EntryState::Granted => Tone::Allowed,
        EntryState::AlwaysDenied | EntryState::Declined => Tone::Denied,
        EntryState::Withdrawn | EntryState::Noted => Tone::Quiet,
    }
}

/// One column of the table. `cell` is a total match over this, so a column added here fails to build until the view says how to draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Question,
    Subject,
    Answer,
    Sandbox,
}

/// What one column holds for one row, carrying what the view needs to draw it rather than a string the view must interpret by position.
#[derive(Debug, PartialEq, Eq)]
pub enum Cell {
    Question(String),
    Subject { text: String, raw: bool },
    Answer { text: String, tone: Tone },
    Sandbox(String),
}

const W_QUESTION: f32 = 104.0;
const W_ANSWER: f32 = 128.0;
const W_SANDBOX: f32 = W_ANSWER;
/// Enough of the subject to read a hostname, whatever the window's width.
const W_SUBJECT_LEAST: f32 = 120.0;

/// The columns the table has. The sandbox is one of them only while the list spans more than one run — narrowed, the sidebar says which.
pub fn columns(selected: Option<&str>) -> Vec<Column> {
    let mut columns = vec![Column::Question, Column::Subject, Column::Answer];
    if selected.is_none() {
        columns.push(Column::Sandbox);
    }
    columns
}

impl Column {
    pub fn head(self) -> &'static str {
        match self {
            Self::Question => "QUESTION",
            Self::Subject => "ASKED ABOUT",
            Self::Answer => "ANSWER",
            Self::Sandbox => "SANDBOX",
        }
    }

    /// This column's own width, or `None` for the one that takes what the others leave.
    fn fixed_width(self) -> Option<f32> {
        match self {
            Self::Question => Some(W_QUESTION),
            Self::Subject => None,
            Self::Answer => Some(W_ANSWER),
            Self::Sandbox => Some(W_SANDBOX),
        }
    }

    /// What this column holds for this row.
    pub fn cell(self, entry: &Entry) -> Cell {
        match self {
            Self::Question => Cell::Question(question(entry)),
            Self::Subject => Cell::Subject {
                text: entry.subject().to_string(),
                raw: is_raw(entry),
            },
            Self::Answer => Cell::Answer {
                text: entry.state.label().to_string(),
                tone: tone(entry),
            },
            Self::Sandbox => Cell::Sandbox(entry.sandbox.clone().unwrap_or_default()),
        }
    }
}

/// Every column's width, in the order [`columns`] gives them, for a row of `available` width whose layout puts `gutter` between each pair of items. The subject takes what the fixed columns and the gutters leave, so the answer never loses its place — or its ellipsis — to a long hostname.
pub fn widths(selected: Option<&str>, available: f32, gutter: f32) -> Vec<f32> {
    let columns = columns(selected);
    let fixed: f32 = columns
        .iter()
        .filter_map(|column| column.fixed_width())
        .sum();
    let gutters = columns.len() as f32 * gutter;
    let subject = (available - fixed - gutters).max(W_SUBJECT_LEAST);
    columns
        .iter()
        .map(|column| column.fixed_width().unwrap_or(subject))
        .collect()
}

/// Whether the open row is one the view still shows, so a filter that still shows it does not throw away a grant half composed on it.
pub fn still_shown(open: Option<&str>, entries: &[Entry], rows: &[usize]) -> bool {
    match open {
        Some(id) => rows
            .iter()
            .filter_map(|i| entries.get(*i))
            .any(|entry| entry.id == id),
        None => false,
    }
}

/// Whether the row the developer opened is still one the list holds. An id is a digest of the question, so a row that goes and is asked again carries the same id — leaving the expansion keyed to it would reopen it with a grant composed before it went.
pub fn still_listed(open: Option<&str>, entries: &[Entry]) -> bool {
    match open {
        Some(id) => entries.iter().any(|entry| entry.id == id),
        None => false,
    }
}

/// What the row asks about, in the words `lns approval ls` prints and the case a row's eyebrow reads in.
pub fn question(entry: &Entry) -> String {
    let asked = crate::approval_flow::answering::kind_of(entry);
    match asked {
        lns_ipc::ApprovalEntryKind::Notice => "NOTICE".to_string(),
        named => named.question().to_uppercase(),
    }
}

/// Whether this row offers the grant the card offered. An answered connector is granted again through `lns connector`, which is where a grant is replaced.
pub fn is_grantable(entry: &Entry) -> bool {
    matches!(entry.kind, EntryKind::Connector { .. })
        && matches!(entry.state, EntryState::Undecided)
}

/// What the view says after a grant. One that landed says nothing — the row reads granted.
pub fn granting_reported(granted: &Granting) -> Option<String> {
    match granted {
        Granting::Granted => None,
        Granting::UnknownId => Some(ENTRY_IS_GONE.to_string()),
        Granting::NotOffered => Some(crate::approval_flow::answering::NOT_OFFERED.to_string()),
    }
}

/// The name the run is known by, or the handle itself when this machine knows no run by it — a sandbox removed since the entry was written still names itself.
fn named<'a>(id: &'a str, sandboxes: &'a [Sandbox]) -> &'a str {
    sandboxes
        .iter()
        .find(|sandbox| sandbox.id == id)
        .map_or(id, |sandbox| sandbox.name.as_str())
}

/// Whether this question still has no answer.
fn waits(entry: &Entry) -> bool {
    entry.is_answerable() && matches!(entry.state, EntryState::Undecided | EntryState::Withdrawn)
}

/// The answers this entry's row offers, leaving out the one it already has and the one that would write nothing.
pub fn offers(entry: &Entry) -> Vec<ApprovalAnswer> {
    if !entry.is_answerable() {
        return Vec::new();
    }
    match entry.state {
        EntryState::Undecided | EntryState::Withdrawn => {
            vec![ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AlwaysDeny]
        }
        EntryState::AlwaysAllowed => vec![ApprovalAnswer::AlwaysDeny, ApprovalAnswer::AskAgain],
        EntryState::AlwaysDenied => vec![ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AskAgain],
        EntryState::Granted | EntryState::Declined | EntryState::Noted => Vec::new(),
    }
}

pub fn action(entry: &Entry) -> Option<&str> {
    match &entry.kind {
        EntryKind::Destination { action, .. } => Some(action),
        EntryKind::Connector { .. } | EntryKind::Notice { .. } => None,
    }
}

/// Whether LNS can read this traffic, which the card says and the row must say too.
pub fn is_raw(entry: &Entry) -> bool {
    matches!(entry.kind, EntryKind::Destination { raw: true, .. })
}

pub fn label(answer: ApprovalAnswer) -> &'static str {
    match answer {
        ApprovalAnswer::AlwaysAllow => "Always allow",
        ApprovalAnswer::AlwaysDeny => "Always deny",
        ApprovalAnswer::AskAgain => "Ask again",
    }
}

const ENTRY_IS_GONE: &str = "that entry is no longer there: the sandbox may have been removed";

/// What the view says after an answer. An answer that wrote a rule says nothing — the row itself now reads differently.
pub fn reported(outcome: &AnswerOutcome) -> Option<String> {
    match outcome {
        AnswerOutcome::Recorded(_) => None,
        AnswerOutcome::UnknownId => Some(ENTRY_IS_GONE.to_string()),
        AnswerOutcome::NotAnswerable => {
            Some(crate::approval_flow::answering::DECIDED_ELSEWHERE.to_string())
        }
        AnswerOutcome::NoRuleWritten(why) => Some(why.clone()),
    }
}

/// What the view says after a removal. A row that went says nothing — it is not there to say it.
pub fn removal_reported(outcome: &RemoveOutcome) -> Option<String> {
    match outcome {
        RemoveOutcome::Removed => None,
        RemoveOutcome::UnknownId => Some(ENTRY_IS_GONE.to_string()),
        RemoveOutcome::NotCleared(reason) => Some(reason.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(run: &str, host: &str, raw: bool, state: EntryState) -> Entry {
        Entry::new(
            Some(run.to_string()),
            EntryKind::Destination {
                destination: host.to_string(),
                action: format!("CONNECT {host}:443"),
                raw,
            },
            state,
        )
    }

    fn connector(run: &str) -> Entry {
        Entry::new(
            Some(run.to_string()),
            EntryKind::Connector {
                name: "linear".into(),
            },
            EntryState::Undecided,
        )
    }

    fn notice(run: &str) -> Entry {
        Entry::new(
            Some(run.to_string()),
            EntryKind::Notice {
                message: "the rule could not be written".into(),
            },
            EntryState::Noted,
        )
    }

    /// No answer chosen, which is every answer.
    fn everything() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn only(answers: [&str; 1]) -> BTreeSet<String> {
        answers.iter().map(|a| (*a).to_string()).collect()
    }

    const ID_A: &str = "deadbeef00000000000000000000aa01";
    const ID_B: &str = "deadbeef00000000000000000000bb02";

    fn sandboxes() -> Vec<Sandbox> {
        vec![
            Sandbox {
                id: ID_A.into(),
                name: "dapper_thistle".into(),
                image: "alpine:latest".into(),
                status: "running".into(),
            },
            Sandbox {
                id: ID_B.into(),
                name: "bold_otter".into(),
                image: "alpine:latest".into(),
                status: "exited".into(),
            },
        ]
    }

    #[test]
    fn choosing_a_sandbox_narrows_the_list_to_what_that_run_was_asked() {
        // The sidebar selects a run by id and an entry is stamped with the run's name, so an unresolved selection matches nothing and the view reads as empty while the run holds questions.
        let held = vec![
            destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided,
            ),
            destination("bold_otter", "api.github.com", false, EntryState::Undecided),
        ];

        assert_eq!(
            listing(&held, Some(ID_A), &sandboxes(), &everything()).rows,
            vec![0]
        );
        assert_eq!(
            listing(&held, None, &sandboxes(), &everything()).rows,
            vec![0, 1],
            "with no sandbox chosen the view spans every run"
        );
    }

    #[test]
    fn a_selection_this_machine_knows_no_run_by_still_matches_what_it_names() {
        // A sandbox removed since the entry was written is gone from the list, and the entry still names it.
        let held = vec![destination(
            "dapper_thistle",
            "api.linear.app",
            false,
            EntryState::Undecided,
        )];

        assert_eq!(
            listing(&held, Some("dapper_thistle"), &[], &everything()).rows,
            vec![0],
            "the handle itself is the last thing left to match on"
        );
    }

    #[test]
    fn the_count_beside_the_view_is_the_questions_still_waiting() {
        // The badge is what makes a closed card recoverable, so it must count what has no answer — and nothing else.
        let held = vec![
            destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided,
            ),
            destination(
                "dapper_thistle",
                "db.internal:5432",
                true,
                EntryState::Withdrawn,
            ),
            destination(
                "dapper_thistle",
                "api.github.com",
                false,
                EntryState::AlwaysAllowed,
            ),
            destination(
                "dapper_thistle",
                "evil.test",
                false,
                EntryState::AlwaysDenied,
            ),
            notice("dapper_thistle"),
            destination("bold_otter", "api.stripe.com", false, EntryState::Undecided),
        ];

        assert_eq!(waiting(&held, None, &sandboxes()), 3);
        assert_eq!(
            waiting(&held, Some(ID_A), &sandboxes()),
            2,
            "the count must describe the sandbox the sidebar named"
        );
    }

    #[test]
    fn choosing_an_answer_narrows_the_list_to_the_rows_that_carry_it() {
        // Coming back to a card you closed means finding the one row that still waits, in a list of every question the run ever asked.
        let held = vec![
            destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided,
            ),
            destination(
                "dapper_thistle",
                "api.github.com",
                false,
                EntryState::AlwaysAllowed,
            ),
            notice("dapper_thistle"),
        ];

        assert_eq!(
            listing(&held, None, &sandboxes(), &only(["undecided"])).rows,
            vec![0]
        );
        assert_eq!(
            listing(&held, None, &sandboxes(), &only(["notice"])).rows,
            vec![2]
        );
        assert_eq!(
            listing(&held, None, &sandboxes(), &everything()).rows,
            vec![0, 1, 2],
            "choosing nothing is choosing every answer, as the audit view's kinds are"
        );
    }

    #[test]
    fn the_chooser_offers_every_answer_a_row_can_carry_and_nothing_else() {
        // An answer the chooser offers that no row carries is a filter that can only empty the list.
        assert_eq!(answers().len(), EntryState::ALL.len());
        for state in EntryState::ALL {
            let label = state.label();
            assert!(
                answers().contains(&label),
                "{state:?} reads as {label:?}, which the chooser does not offer"
            );
        }
    }

    #[test]
    fn an_answer_reads_as_loudly_as_what_it_permits() {
        // The answer is what the developer came back for; allow and deny must not look alike at a glance.
        assert_eq!(
            tone(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided
            )),
            Tone::Waiting
        );
        assert_eq!(
            tone(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::AlwaysAllowed
            )),
            Tone::Allowed
        );
        assert_eq!(
            tone(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::AlwaysDenied
            )),
            Tone::Denied
        );
        assert_eq!(
            tone(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Withdrawn
            )),
            Tone::Quiet,
            "a workload that exited decided nothing, so its row must not read as an answer"
        );
        assert_eq!(tone(&notice("dapper_thistle")), Tone::Quiet);
        assert_eq!(
            tone(&Entry::new(
                Some("dapper_thistle".to_string()),
                EntryKind::Connector {
                    name: "linear".into()
                },
                EntryState::Granted,
            )),
            Tone::Allowed
        );
        assert_eq!(
            tone(&Entry::new(
                Some("dapper_thistle".to_string()),
                EntryKind::Connector {
                    name: "linear".into()
                },
                EntryState::Declined,
            )),
            Tone::Denied
        );
    }

    #[test]
    fn every_column_holds_what_it_says_it_holds() {
        // The sandbox reads whole: two runs can be asked about one host, and a shortened name is one nobody can answer safely.
        let entry = destination(
            "dapper_thistle",
            "db.internal:5432",
            true,
            EntryState::AlwaysAllowed,
        );

        let held: Vec<Cell> = columns(None)
            .into_iter()
            .map(|column| column.cell(&entry))
            .collect();

        assert_eq!(
            held,
            vec![
                Cell::Question("DESTINATION".to_string()),
                Cell::Subject {
                    text: "db.internal:5432".to_string(),
                    raw: true,
                },
                Cell::Answer {
                    text: "always allow".to_string(),
                    tone: Tone::Allowed,
                },
                Cell::Sandbox("dapper_thistle".to_string()),
            ]
        );
    }

    #[test]
    fn narrowing_to_one_sandbox_drops_the_column_the_sidebar_names() {
        assert_eq!(
            columns(Some(ID_A)),
            vec![Column::Question, Column::Subject, Column::Answer]
        );
        assert_eq!(
            columns(None).len(),
            widths(None, 800.0, 10.0).len(),
            "a column with no width would leave the heads above nothing"
        );
    }

    #[test]
    fn every_head_names_its_own_column() {
        for column in columns(None) {
            assert!(!column.head().is_empty(), "{column:?}");
        }
        assert_eq!(Column::Subject.head(), "ASKED ABOUT");
    }

    #[test]
    fn the_columns_and_the_gutters_between_them_fit_the_row() {
        // A row that overruns clips its last column mid-glyph, with no ellipsis to say it was cut — and the last column is the sandbox, or the answer once the list is narrowed.
        let gutter = 10.0;
        let wide = widths(None, 800.0, gutter);

        assert_eq!(
            wide.iter().sum::<f32>() + wide.len() as f32 * gutter,
            800.0,
            "the widths plus the gutters are the whole row"
        );
        assert_eq!(
            wide[2], wide[3],
            "the sandbox reads in the answer's own width"
        );
        assert!(wide[1] > wide[0], "the subject is the column that grows");
        assert_eq!(
            widths(Some(ID_A), 800.0, gutter).len(),
            3,
            "narrowed, there is one column fewer to fit"
        );

        let subject = widths(None, 100.0, gutter)[1];
        assert!(
            subject >= 120.0,
            "a window too small to fit the table still shows enough subject to read a hostname, got {subject}"
        );
    }

    #[test]
    fn an_entry_no_run_stamped_still_fills_its_sandbox_cell() {
        // A persisted list that omits the field reads back as no run, and a missing cell would put every later column under the wrong head.
        let unstamped = Entry::new(
            None,
            EntryKind::Notice {
                message: "the rule could not be written".into(),
            },
            EntryState::Noted,
        );

        assert_eq!(
            Column::Sandbox.cell(&unstamped),
            Cell::Sandbox(String::new())
        );
    }

    #[test]
    fn a_row_a_filter_hides_is_no_longer_open_and_one_it_keeps_stays() {
        // Closing a row throws away the grant being composed on it, so a filter that still shows the row must leave it alone.
        let held = vec![
            destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided,
            ),
            destination("bold_otter", "api.stripe.com", false, EntryState::Undecided),
        ];

        assert!(still_shown(Some(&held[0].id), &held, &[0, 1]));
        assert!(still_shown(Some(&held[0].id), &held, &[0]));
        assert!(!still_shown(Some(&held[0].id), &held, &[1]));
        assert!(!still_shown(None, &held, &[0, 1]));
    }

    #[test]
    fn a_row_that_left_the_list_is_no_longer_open() {
        // Removing a row is done from inside its own expansion, and an id is a digest of the question, so the same question asked again would reopen with the grant composed before it went.
        let held = vec![destination(
            "dapper_thistle",
            "api.linear.app",
            false,
            EntryState::Undecided,
        )];

        assert!(still_listed(Some(&held[0].id), &held));
        assert!(!still_listed(Some(&held[0].id), &[]));
        assert!(!still_listed(None, &held), "nothing open is not listed");
    }

    #[test]
    fn every_row_says_what_it_asks_about() {
        // A connector shown by name alone reads as a destination nobody can answer, which is what the developer reported.
        assert_eq!(
            question(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided
            )),
            "DESTINATION"
        );
        assert_eq!(question(&connector("dapper_thistle")), "CONNECTOR");
        assert_eq!(
            question(&notice("dapper_thistle")),
            "NOTICE",
            "a notice names no question at a terminal, and the row still has to say what it is"
        );
    }

    #[test]
    fn only_an_unanswered_connector_row_offers_the_grant() {
        // A grant already given is replaced through `lns connector`, which is where a grant's own verbs live.
        assert!(is_grantable(&connector("dapper_thistle")));
        for state in [EntryState::Granted, EntryState::Declined] {
            assert!(
                !is_grantable(&Entry::new(
                    Some("dapper_thistle".to_string()),
                    EntryKind::Connector {
                        name: "linear".into()
                    },
                    state,
                )),
                "{state:?}"
            );
        }
        assert!(
            !is_grantable(&destination(
                "dapper_thistle",
                "api.linear.app",
                false,
                EntryState::Undecided
            )),
            "a destination is answered, not granted"
        );
    }

    #[test]
    fn a_grant_that_landed_reports_nothing_and_every_refusal_says_why() {
        assert_eq!(granting_reported(&Granting::Granted), None);
        assert!(
            granting_reported(&Granting::UnknownId).is_some_and(|said| said.contains("no longer"))
        );
        assert_eq!(
            granting_reported(&Granting::NotOffered).as_deref(),
            Some(crate::approval_flow::answering::NOT_OFFERED),
            "a sandbox that is no longer holding the offer must say where the grant goes instead"
        );
    }

    #[test]
    fn an_unanswered_destination_offers_the_two_answers_that_write_a_rule() {
        // `ask again` takes a rule back, and an entry that never wrote one has nothing to take, so offering it would only ever refuse.
        for state in [EntryState::Undecided, EntryState::Withdrawn] {
            assert_eq!(
                offers(&destination("aa01", "api.linear.app", false, state)),
                vec![ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AlwaysDeny],
                "{state:?}"
            );
        }
    }

    #[test]
    fn an_answered_destination_offers_the_other_verdict_and_asking_again() {
        assert_eq!(
            offers(&destination(
                "aa01",
                "api.linear.app",
                false,
                EntryState::AlwaysAllowed
            )),
            vec![ApprovalAnswer::AlwaysDeny, ApprovalAnswer::AskAgain],
            "the answer it already has is not offered again"
        );
        assert_eq!(
            offers(&destination(
                "aa01",
                "api.linear.app",
                false,
                EntryState::AlwaysDenied
            )),
            vec![ApprovalAnswer::AlwaysAllow, ApprovalAnswer::AskAgain]
        );
    }

    #[test]
    fn a_connector_and_a_notice_offer_nothing() {
        assert!(offers(&notice("aa01")).is_empty());
        assert!(
            offers(&Entry::new(
                Some("aa01".to_string()),
                EntryKind::Connector {
                    name: "linear".into()
                },
                EntryState::Granted,
            ))
            .is_empty(),
            "a connector is connected and granted through `lns connector`"
        );
    }

    #[test]
    fn a_state_a_destination_cannot_reach_offers_nothing_rather_than_a_guess() {
        // Nothing writes this pairing, and a hand-edited approvals file is the ordinary way to get one; a button here would answer a question we cannot describe.
        assert!(
            offers(&destination(
                "aa01",
                "api.linear.app",
                false,
                EntryState::Granted
            ))
            .is_empty()
        );
    }

    #[test]
    fn a_destination_row_carries_the_action_the_card_showed_and_says_when_it_is_raw() {
        let inspected = destination("aa01", "api.linear.app", false, EntryState::Undecided);
        assert_eq!(action(&inspected), Some("CONNECT api.linear.app:443"));
        assert!(!is_raw(&inspected));

        let raw = destination("aa01", "db.internal:5432", true, EntryState::Undecided);
        assert!(
            is_raw(&raw),
            "the view must mark what LNS cannot inspect, as the card does"
        );
        assert_eq!(action(&notice("aa01")), None);
        assert!(!is_raw(&notice("aa01")));
    }

    #[test]
    fn every_answer_reads_as_the_words_the_terminal_uses() {
        assert_eq!(label(ApprovalAnswer::AlwaysAllow), "Always allow");
        assert_eq!(label(ApprovalAnswer::AlwaysDeny), "Always deny");
        assert_eq!(label(ApprovalAnswer::AskAgain), "Ask again");
    }

    #[test]
    fn a_removal_reports_nothing_and_every_refusal_says_why() {
        assert_eq!(removal_reported(&RemoveOutcome::Removed), None);
        assert!(
            removal_reported(&RemoveOutcome::UnknownId)
                .is_some_and(|said| said.contains("no longer")),
        );
        assert_eq!(
            removal_reported(&RemoveOutcome::NotCleared(
                "this run's approvals could not be read".to_string()
            ))
            .as_deref(),
            Some("this run's approvals could not be read"),
            "a removal that did not land must not read as one that did"
        );
    }

    #[test]
    fn a_recorded_answer_reports_nothing_and_every_refusal_says_why() {
        assert_eq!(
            reported(&AnswerOutcome::Recorded(EntryState::AlwaysAllowed)),
            None
        );
        assert_eq!(
            reported(&AnswerOutcome::NoRuleWritten("no rule of its own".into())).as_deref(),
            Some("no rule of its own"),
            "the reason the service gave is the reason the window shows"
        );
        assert_eq!(
            reported(&AnswerOutcome::NotAnswerable).as_deref(),
            Some(crate::approval_flow::answering::DECIDED_ELSEWHERE)
        );
        assert!(
            reported(&AnswerOutcome::UnknownId)
                .is_some_and(|said| said.contains("no longer") || said.contains("gone")),
            "an entry removed while the window was open must say so, not fail silently"
        );
    }
}
