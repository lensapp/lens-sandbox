use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum EntryKind {
    Destination {
        destination: String,
        action: String,
        raw: bool,
    },
    Connector {
        name: String,
    },
    Notice {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryState {
    Undecided,
    Withdrawn,
    AlwaysAllowed,
    AlwaysDenied,
    Granted,
    Declined,
    Noted,
}

impl EntryState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Undecided => "undecided",
            Self::Withdrawn => "withdrawn",
            Self::AlwaysAllowed => "always allow",
            Self::AlwaysDenied => "always deny",
            Self::Granted => "granted",
            Self::Declined => "declined",
            Self::Noted => "notice",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub sandbox: Option<String>,
    pub kind: EntryKind,
    pub state: EntryState,
}

impl Entry {
    pub fn new(sandbox: Option<String>, kind: EntryKind, state: EntryState) -> Self {
        Self {
            id: identity(sandbox.as_deref(), &kind),
            sandbox,
            kind,
            state,
        }
    }

    /// What the entry was asked about, as `lns approval ls` prints it.
    pub fn subject(&self) -> &str {
        match &self.kind {
            EntryKind::Destination { destination, .. } => destination,
            EntryKind::Connector { name } => name,
            EntryKind::Notice { message } => message,
        }
    }

    /// Whether the run has an answer for this question that outlives the request, so a later card must not report it as unanswered.
    pub fn is_settled(&self) -> bool {
        matches!(
            self.state,
            EntryState::AlwaysAllowed | EntryState::AlwaysDenied
        )
    }

    /// Whether this entry is one an answer can decide, as opposed to a notice or a connector the run already granted.
    pub fn is_answerable(&self) -> bool {
        matches!(self.kind, EntryKind::Destination { .. })
    }
}

/// The same question asked twice is one entry, so identity is a collision-resistant digest of what was asked — never the guest's request id, which no restart preserves and which a workload could aim at an answer already given.
fn identity(sandbox: Option<&str>, kind: &EntryKind) -> String {
    let run = sandbox.unwrap_or_default();
    let key = match kind {
        EntryKind::Destination {
            destination, raw, ..
        } => format!("d\u{1}{run}\u{1}{destination}\u{1}{raw}"),
        EntryKind::Connector { name } => format!("c\u{1}{run}\u{1}{name}"),
        EntryKind::Notice { message } => format!("n\u{1}{run}\u{1}{message}"),
    };
    let digest = <sha2::Sha256 as sha2::Digest>::digest(key.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub trait EntryStore: Send + Sync {
    fn record(&self, entry: Entry);
    fn list(&self) -> Vec<Entry>;
}

/// The store a session runs with when nothing keeps its entries — a host with no run directory, and every test that does not ask about them.
pub struct NoEntryStore;

impl EntryStore for NoEntryStore {
    fn record(&self, _: Entry) {}
    fn list(&self) -> Vec<Entry> {
        Vec::new()
    }
}

pub trait Fs: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

pub struct RealFs;

impl Fs for RealFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        lns_policy::secure_file::write_json_secret_atomic(path, bytes)
    }
}

pub struct FileEntryStore {
    path: PathBuf,
    fs: Arc<dyn Fs>,
    write_lock: Mutex<()>,
}

impl FileEntryStore {
    pub fn new(path: PathBuf) -> Self {
        Self::with_fs(path, Arc::new(RealFs))
    }

    pub fn with_fs(path: PathBuf, fs: Arc<dyn Fs>) -> Self {
        Self {
            path,
            fs,
            write_lock: Mutex::new(()),
        }
    }

    /// A file that is missing, or not the JSON we wrote, reads as no entries: a run whose list we cannot parse must still raise cards.
    fn read_all(&self) -> Held {
        match self.fs.read(&self.path) {
            Ok(bytes) => Held::Read(self.parse(&bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Held::Read(Vec::new()),
            // Anything else is a file we may not overwrite: it holds entries we cannot see.
            Err(e) => {
                crate::log::warn!("could not read the approvals at {:?}: {e}", self.path);
                Held::Unreadable
            }
        }
    }

    fn parse(&self, bytes: &[u8]) -> Vec<Entry> {
        match serde_json::from_slice(bytes) {
            Ok(entries) => entries,
            Err(e) => {
                crate::log::warn!("ignoring unreadable approvals at {:?}: {e}", self.path);
                Vec::new()
            }
        }
    }

    fn write_all(&self, entries: &[Entry]) {
        if let Err(e) = self.render_and_write(entries) {
            crate::log::warn!("could not record the approval at {:?}: {e}", self.path);
        }
    }

    fn render_and_write(&self, entries: &[Entry]) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(entries)?;
        self.fs.write(&self.path, &bytes)
    }
}

impl EntryStore for FileEntryStore {
    fn record(&self, entry: Entry) {
        let _guard = self.write_lock.lock().expect("approvals mutex poisoned");
        let Held::Read(mut entries) = self.read_all() else {
            return;
        };
        match entries.iter_mut().find(|held| held.id == entry.id) {
            Some(held) => *held = entry,
            None => entries.push(entry),
        }
        self.write_all(&entries);
    }

    fn list(&self) -> Vec<Entry> {
        match self.read_all() {
            Held::Read(entries) => entries,
            Held::Unreadable => Vec::new(),
        }
    }
}

/// What the file holds, or that we could not tell — a file we cannot read must not be written over with what we happen to know.
enum Held {
    Read(Vec<Entry>),
    Unreadable,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct FakeFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        write_fails: Mutex<bool>,
        read_fails: Mutex<bool>,
    }

    impl FakeFs {
        fn seed(&self, path: &Path, bytes: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), bytes.to_vec());
        }

        fn break_writes(&self) {
            *self.write_fails.lock().unwrap() = true;
        }

        fn break_reads(&self) {
            *self.read_fails.lock().unwrap() = true;
        }

        fn allow_reads(&self) {
            *self.read_fails.lock().unwrap() = false;
        }

        fn contents(&self, path: &Path) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(path).cloned()
        }
    }

    impl Fs for FakeFs {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            if *self.read_fails.lock().unwrap() {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            if *self.write_fails.lock().unwrap() {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.seed(path, bytes);
            Ok(())
        }
    }

    fn destination(host: &str) -> EntryKind {
        EntryKind::Destination {
            destination: host.to_string(),
            action: format!("CONNECT {host}:443"),
            raw: false,
        }
    }

    fn fixture() -> (FileEntryStore, Arc<FakeFs>) {
        let fs = Arc::new(FakeFs::default());
        let store = FileEntryStore::with_fs(PathBuf::from("/run/approvals.json"), fs.clone());
        (store, fs)
    }

    #[test]
    fn a_recorded_entry_is_listed_back() {
        let (store, _fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].subject(), "api.linear.app");
        assert_eq!(listed[0].state, EntryState::Undecided);
        assert_eq!(listed[0].sandbox.as_deref(), Some("reviewer"));
    }

    #[test]
    fn the_same_question_asked_twice_stays_one_entry() {
        // The guest gives every request its own id, so keying by that would list one destination once per attempt.
        let (store, _fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::AlwaysAllowed,
        ));

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, EntryState::AlwaysAllowed);
    }

    #[test]
    fn two_sandboxes_asking_the_same_question_keep_their_own_entries() {
        let (store, _fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));
        store.record(Entry::new(
            Some("auditor".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn a_raw_splice_is_a_different_entry_from_the_host_it_names() {
        let (store, _fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("db.internal"),
            EntryState::Undecided,
        ));
        store.record(Entry::new(
            Some("reviewer".into()),
            EntryKind::Destination {
                destination: "db.internal".into(),
                action: "SPLICE db.internal:5432".into(),
                raw: true,
            },
            EntryState::Undecided,
        ));

        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn two_destinations_never_share_an_id() {
        // The workload picks the hostname, and one entry replaces another of the same id, so a short hash would let it erase an answer the developer gave.
        let (store, _fs) = fixture();
        for host in ["ypdhkx.example", "wifyjs.example"] {
            store.record(Entry::new(
                Some("aa01".into()),
                destination(host),
                EntryState::Undecided,
            ));
        }

        assert_eq!(
            store.list().len(),
            2,
            "these two hostnames collide under a 32-bit hash"
        );
    }

    #[test]
    fn an_entry_keeps_its_id_across_a_restart() {
        // The id is what `lns approval answer` names, so it has to survive the process that printed it.
        let (store, fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));
        let before = store.list()[0].id.clone();

        let restarted = FileEntryStore::with_fs(PathBuf::from("/run/approvals.json"), fs);

        assert_eq!(restarted.list()[0].id, before);
    }

    #[test]
    fn a_run_with_no_approvals_file_yet_lists_nothing() {
        let (store, _fs) = fixture();
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_file_that_is_not_the_json_we_wrote_lists_nothing_rather_than_failing() {
        let (store, fs) = fixture();
        fs.seed(Path::new("/run/approvals.json"), b"{ not json");

        assert!(store.list().is_empty());
    }

    #[test]
    fn a_recording_over_a_file_we_cannot_read_writes_nothing() {
        // Reading nothing off an unreadable file and then saving would replace every entry the run kept with the one card in hand.
        let (store, fs) = fixture();
        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.github.com"),
            EntryState::AlwaysAllowed,
        ));
        let kept = fs.contents(Path::new("/run/approvals.json"));
        fs.break_reads();

        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        fs.allow_reads();
        assert_eq!(
            fs.contents(Path::new("/run/approvals.json")),
            kept,
            "the file must be left for a reader that can see it"
        );
    }

    #[test]
    fn a_recording_that_cannot_be_written_leaves_the_run_asking() {
        // The card is already up and the request is already held; a store that cannot write must not take the session down with it.
        let (store, fs) = fixture();
        fs.break_writes();

        store.record(Entry::new(
            None,
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        assert!(store.list().is_empty());
    }

    #[test]
    fn a_notice_and_a_connector_are_not_answerable_but_a_destination_is() {
        let asked = Entry::new(None, destination("api.linear.app"), EntryState::Undecided);
        let granted = Entry::new(
            None,
            EntryKind::Connector {
                name: "linear".into(),
            },
            EntryState::Granted,
        );
        let told = Entry::new(
            None,
            EntryKind::Notice {
                message: "something happened".into(),
            },
            EntryState::Noted,
        );

        assert!(asked.is_answerable());
        assert!(!asked.is_settled());
        assert!(!granted.is_answerable());
        assert!(!told.is_answerable());
        assert_eq!(granted.subject(), "linear");
        assert_eq!(told.subject(), "something happened");
    }

    #[test]
    fn every_state_prints_the_words_the_list_shows() {
        assert_eq!(EntryState::Undecided.label(), "undecided");
        assert_eq!(EntryState::Withdrawn.label(), "withdrawn");
        assert_eq!(EntryState::AlwaysAllowed.label(), "always allow");
        assert_eq!(EntryState::AlwaysDenied.label(), "always deny");
        assert_eq!(EntryState::Granted.label(), "granted");
        assert_eq!(EntryState::Declined.label(), "declined");
        assert_eq!(EntryState::Noted.label(), "notice");
    }

    #[test]
    fn the_store_that_keeps_nothing_answers_every_call() {
        // Every session holds a store; one built where no run directory exists must be safe to call, not a branch each caller checks.
        let store = NoEntryStore;
        store.record(Entry::new(
            None,
            destination("api.linear.app"),
            EntryState::Undecided,
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn the_real_filesystem_writes_and_reads_one_run_s_approvals() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = FileEntryStore::new(dir.path().join("approvals.json"));

        store.record(Entry::new(
            Some("reviewer".into()),
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        assert_eq!(store.list()[0].subject(), "api.linear.app");
    }

    #[test]
    fn the_real_filesystem_reports_a_directory_it_cannot_write() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let occupied = dir.path().join("approvals.json");
        std::fs::create_dir(&occupied).expect("occupy the path");

        let store = FileEntryStore::new(occupied);
        store.record(Entry::new(
            None,
            destination("api.linear.app"),
            EntryState::Undecided,
        ));

        assert!(store.list().is_empty());
    }
}
