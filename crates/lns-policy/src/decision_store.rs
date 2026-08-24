//! One per-machine decision file: a JSON map from a decision's key to what the developer answered. Each answer is a risk one person accepted on one computer, so these files are never committed and never travel with a document.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub type DecisionFile<T> = HashMap<String, T>;

pub trait DecisionStore<T>: Send + Sync {
    fn load(&self) -> io::Result<DecisionFile<T>>;
    fn save(&self, state: &DecisionFile<T>) -> io::Result<()>;
}

pub struct JsonDecisionStore<T> {
    pub path: PathBuf,
    answer: PhantomData<fn() -> T>,
}

impl<T> JsonDecisionStore<T> {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            answer: PhantomData,
        }
    }
}

impl<T: Serialize + DeserializeOwned + Send + Sync> DecisionStore<T> for JsonDecisionStore<T> {
    fn load(&self) -> io::Result<DecisionFile<T>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &DecisionFile<T>) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum SomeAnswer {
        Yes,
        No,
    }

    fn store(dir: &tempfile::TempDir, name: &str) -> JsonDecisionStore<SomeAnswer> {
        JsonDecisionStore::new(dir.path().join(name))
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(store(&dir, "never-created.json").load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("decisions.json"), "{ not json").unwrap();
        assert_eq!(
            store(&dir, "decisions.json").load().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store: JsonDecisionStore<SomeAnswer> = JsonDecisionStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = store(&dir, "decisions.json");
        let mut original = DecisionFile::new();
        original.insert("first".to_string(), SomeAnswer::Yes);
        original.insert("second".to_string(), SomeAnswer::No);
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn save_replaces_the_whole_file_rather_than_merging() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = store(&dir, "decisions.json");
        let mut first = DecisionFile::new();
        first.insert("first".to_string(), SomeAnswer::Yes);
        store.save(&first).unwrap();
        let mut second = DecisionFile::new();
        second.insert("second".to_string(), SomeAnswer::No);
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap(), second);
    }
}
