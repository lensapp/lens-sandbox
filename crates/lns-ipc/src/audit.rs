use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Error)]
pub enum AuditChainError {
    #[error("parsing audit-event JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("audit event must be a JSON object")]
    NotAnObject,
}

#[derive(Debug, Default)]
pub struct AuditChain {
    prev_hash: Option<String>,
    line_count: u64,
}

impl AuditChain {
    pub fn new() -> Self {
        Self {
            prev_hash: None,
            line_count: 0,
        }
    }

    pub fn resuming_from(prev_hash: Option<String>) -> Self {
        Self {
            prev_hash,
            line_count: 0,
        }
    }

    pub fn resuming_from_anchor(anchor: Option<&Anchor>) -> Self {
        match anchor {
            Some(a) => Self {
                prev_hash: Some(a.head_hash.clone()),
                line_count: a.line_count,
            },
            None => Self::new(),
        }
    }

    pub fn augment(&mut self, raw_json: &str) -> Result<Vec<u8>, AuditChainError> {
        let value: Value = serde_json::from_str(raw_json)?;
        match value {
            Value::Object(obj) => Ok(self.augment_obj(obj)),
            _ => Err(AuditChainError::NotAnObject),
        }
    }

    pub fn augment_obj(&mut self, mut obj: serde_json::Map<String, Value>) -> Vec<u8> {
        let prev = self
            .prev_hash
            .clone()
            .unwrap_or_else(|| GENESIS_PREV_HASH.to_string());
        obj.insert("prev_hash".to_string(), Value::String(prev));
        let bytes = serde_json::to_vec(&Value::Object(obj))
            .expect("serde_json::Value re-serializes infallibly");
        self.prev_hash = Some(hex_encode(&Sha256::digest(&bytes)));
        self.line_count += 1;
        bytes
    }

    pub fn head(&self) -> Option<&str> {
        self.prev_hash.as_deref()
    }

    pub fn line_count(&self) -> u64 {
        self.line_count
    }

    pub fn anchor(&self) -> Option<Anchor> {
        Some(Anchor {
            head_hash: self.prev_hash.clone()?,
            line_count: self.line_count,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    pub head_hash: String,
    pub line_count: u64,
}

impl Anchor {
    pub fn to_line(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).expect("Anchor re-serializes infallibly");
        bytes.push(b'\n');
        bytes
    }

    pub fn parse(text: &str) -> Result<Self, AuditChainError> {
        Ok(serde_json::from_str(text.trim())?)
    }
}

pub fn line_hash(line: &[u8]) -> String {
    hex_encode(&Sha256::digest(line))
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_carries_genesis_prev_hash() {
        let mut c = AuditChain::new();
        let bytes = c.augment(r#"{"type":"audit_event","seq":1}"#).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains(&format!("\"prev_hash\":\"{GENESIS_PREV_HASH}\"")));
    }

    #[test]
    fn second_line_prev_hash_matches_first_line_sha() {
        let mut c = AuditChain::new();
        let line1 = c.augment(r#"{"type":"audit_event","seq":1}"#).unwrap();
        let want_prev = hex_encode(&Sha256::digest(&line1));
        let line2 = c.augment(r#"{"type":"audit_event","seq":2}"#).unwrap();
        let s = std::str::from_utf8(&line2).unwrap();
        assert!(s.contains(&format!("\"prev_hash\":\"{want_prev}\"")));
    }

    #[test]
    fn augment_overwrites_supplied_prev_hash() {
        let mut c = AuditChain::new();
        let bytes = c
            .augment(r#"{"type":"audit_event","seq":1,"prev_hash":"deadbeef"}"#)
            .unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains(&format!("\"prev_hash\":\"{GENESIS_PREV_HASH}\"")));
        assert!(!s.contains("deadbeef"));
    }

    #[test]
    fn rejects_non_object_json() {
        let mut c = AuditChain::new();
        let err = c.augment(r#""just a string""#).unwrap_err();
        assert!(matches!(err, AuditChainError::NotAnObject));
    }

    #[test]
    fn rejects_invalid_json() {
        let mut c = AuditChain::new();
        let err = c.augment("not json at all").unwrap_err();
        assert!(matches!(err, AuditChainError::Parse(_)));
    }

    #[test]
    fn head_is_none_before_any_augment() {
        let c = AuditChain::new();
        assert!(c.head().is_none());
    }

    #[test]
    fn resuming_from_continues_chain_from_supplied_hash() {
        let mut c = AuditChain::resuming_from(Some("abc123".to_string()));
        let bytes = c.augment(r#"{"x":1}"#).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("\"prev_hash\":\"abc123\""));
    }

    #[test]
    fn line_hash_of_a_written_line_equals_the_chain_head_after_writing_it() {
        let mut c = AuditChain::new();
        let line = c.augment(r#"{"x":1}"#).unwrap();
        assert_eq!(line_hash(&line), c.head().unwrap());
    }

    #[test]
    fn head_advances_per_augment() {
        let mut c = AuditChain::new();
        c.augment(r#"{"a":1}"#).unwrap();
        let h1 = c.head().unwrap().to_string();
        c.augment(r#"{"a":2}"#).unwrap();
        let h2 = c.head().unwrap().to_string();
        assert_ne!(h1, h2);
    }

    #[test]
    fn line_count_advances_per_augment() {
        let mut c = AuditChain::new();
        assert_eq!(c.line_count(), 0);
        c.augment(r#"{"a":1}"#).unwrap();
        assert_eq!(c.line_count(), 1);
        c.augment(r#"{"a":2}"#).unwrap();
        assert_eq!(c.line_count(), 2);
    }

    #[test]
    fn anchor_is_none_before_any_augment() {
        assert!(AuditChain::new().anchor().is_none());
    }

    #[test]
    fn anchor_carries_head_and_line_count() {
        let mut c = AuditChain::new();
        c.augment(r#"{"a":1}"#).unwrap();
        c.augment(r#"{"a":2}"#).unwrap();
        let a = c.anchor().expect("anchor after augment");
        assert_eq!(a.head_hash, c.head().unwrap());
        assert_eq!(a.line_count, 2);
    }

    #[test]
    fn resuming_from_anchor_restores_head_and_count() {
        let anchor = Anchor {
            head_hash: "abc123".to_string(),
            line_count: 5,
        };
        let mut c = AuditChain::resuming_from_anchor(Some(&anchor));
        assert_eq!(c.line_count(), 5);
        let bytes = c.augment(r#"{"x":1}"#).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("\"prev_hash\":\"abc123\""));
        assert_eq!(c.line_count(), 6);
    }

    #[test]
    fn resuming_from_anchor_none_starts_fresh_at_genesis() {
        let mut c = AuditChain::resuming_from_anchor(None);
        assert_eq!(c.line_count(), 0);
        let bytes = c.augment(r#"{"x":1}"#).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains(&format!("\"prev_hash\":\"{GENESIS_PREV_HASH}\"")));
    }

    #[test]
    fn anchor_round_trips_through_a_line() {
        let a = Anchor {
            head_hash: "deadbeef".to_string(),
            line_count: 42,
        };
        let line = a.to_line();
        assert!(line.ends_with(b"\n"));
        let parsed = Anchor::parse(std::str::from_utf8(&line).unwrap()).unwrap();
        assert_eq!(parsed, a);
    }

    #[test]
    fn anchor_parse_rejects_non_anchor_json() {
        assert!(Anchor::parse(r#"{"unrelated":true}"#).is_err());
    }
}
