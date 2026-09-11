use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pass,
    Fail,
    Skip,
    BlockedByProduct,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Skip => "skip",
            Status::BlockedByProduct => "blocked-by-product",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Measure {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl From<bool> for Measure {
    fn from(value: bool) -> Self {
        Measure::Bool(value)
    }
}

impl From<u64> for Measure {
    fn from(value: u64) -> Self {
        Measure::Int(value as i64)
    }
}

impl From<i64> for Measure {
    fn from(value: i64) -> Self {
        Measure::Int(value)
    }
}

impl From<f64> for Measure {
    fn from(value: f64) -> Self {
        Measure::Float(value)
    }
}

impl From<String> for Measure {
    fn from(value: String) -> Self {
        Measure::Text(value)
    }
}

impl From<&str> for Measure {
    fn from(value: &str) -> Self {
        Measure::Text(value.to_string())
    }
}

impl std::fmt::Display for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Measure::Bool(value) => write!(f, "{value}"),
            Measure::Int(value) => write!(f, "{value}"),
            Measure::Float(value) => write!(f, "{value:.3}"),
            Measure::Text(value) => write!(f, "{value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub name: String,
    pub status: Status,
    pub duration_ms: u64,
    #[serde(default)]
    pub measures: BTreeMap<String, Measure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl CaseResult {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: Status::Skip,
            duration_ms: 0,
            measures: BTreeMap::new(),
            error: None,
            reason: None,
        }
    }

    pub fn record(&mut self, key: &str, value: impl Into<Measure>) {
        self.measures.insert(key.to_string(), value.into());
    }

    pub fn pass(mut self) -> Self {
        self.status = Status::Pass;
        self
    }

    pub fn fail(mut self, error: impl std::fmt::Display) -> Self {
        self.status = Status::Fail;
        self.error = Some(error.to_string());
        self
    }

    pub fn skip(mut self, reason: impl std::fmt::Display) -> Self {
        self.status = Status::Skip;
        self.reason = Some(reason.to_string());
        self
    }

    pub fn blocked(mut self, reason: impl std::fmt::Display) -> Self {
        self.status = Status::BlockedByProduct;
        self.reason = Some(reason.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryRecord {
    pub role: String,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostFacts {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_scope_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendRecord {
    pub name: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub expectations: BTreeMap<String, String>,
    #[serde(default)]
    pub expected_differences: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_kib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_fds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub schema_version: u32,
    pub harness_revision: String,
    pub backend: BackendRecord,
    pub binaries: Vec<BinaryRecord>,
    pub lns_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_pid: Option<u32>,
    #[serde(default)]
    pub images: Vec<ImageRecord>,
    pub host: HostFacts,
    pub started_unix_ms: u64,
    pub finished_unix_ms: u64,
    #[serde(default)]
    pub cases: Vec<CaseResult>,
    #[serde(default)]
    pub samples: Vec<Sample>,
}

impl RunResult {
    pub fn case(&self, name: &str) -> Option<&CaseResult> {
        self.cases.iter().find(|c| c.name == name)
    }

    pub fn expects_difference(&self, case: &str) -> bool {
        self.backend
            .expected_differences
            .iter()
            .any(|name| name == case)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).with_context(|| format!("write {}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let parsed: RunResult = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a parity result", path.display()))?;
        if parsed.schema_version != SCHEMA_VERSION {
            anyhow::bail!(
                "{} carries schema version {}, this harness reads {SCHEMA_VERSION}",
                path.display(),
                parsed.schema_version
            );
        }
        Ok(parsed)
    }

    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for case in &self.cases {
            *counts.entry(case.status.as_str()).or_insert(0) += 1;
        }
        counts
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> RunResult {
        RunResult {
            schema_version: SCHEMA_VERSION,
            harness_revision: "abc1234".into(),
            backend: BackendRecord {
                name: "netstack".into(),
                env: BTreeMap::from([("LNS_NETDEV".into(), "netstack".into())]),
                expectations: BTreeMap::from([("loopback-witness".into(), "refused".into())]),
                expected_differences: vec!["lease-and-resolver".into()],
            },
            binaries: vec![BinaryRecord {
                role: "lns".into(),
                path: "/tmp/lns".into(),
                sha256: "00".into(),
            }],
            lns_version: "lns 0.25.0".into(),
            service_pid: Some(4242),
            images: vec![ImageRecord {
                reference: "docker.io/library/alpine:3.20".into(),
                digest: Some("sha256:dead".into()),
            }],
            host: HostFacts {
                os: "macos".into(),
                os_version: "15.5".into(),
                arch: "arm64".into(),
                dns_scope_count: Some(6),
            },
            started_unix_ms: 1,
            finished_unix_ms: 2,
            cases: vec![CaseResult::new("upload-100m").pass()],
            samples: vec![Sample {
                at_ms: 5000,
                rss_kib: Some(120_000),
                open_fds: Some(48),
            }],
        }
    }

    #[test]
    fn a_result_round_trips_through_the_file_the_diff_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let result = minimal();
        result.write(&path).unwrap();

        assert_eq!(RunResult::read(&path).unwrap(), result);
    }

    #[test]
    fn a_result_from_another_schema_version_is_refused_rather_than_compared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let mut result = minimal();
        result.schema_version = SCHEMA_VERSION + 1;
        result.write(&path).unwrap();

        let err = RunResult::read(&path).unwrap_err().to_string();
        assert!(err.contains("schema version"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_a_result_names_itself_in_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.json");
        std::fs::write(&path, "{}").unwrap();

        let err = format!("{:#}", RunResult::read(&path).unwrap_err());
        assert!(err.contains("notes.json"), "{err}");
    }

    #[test]
    fn each_status_keeps_the_spelling_the_report_and_the_diff_print() {
        let statuses = [
            (Status::Pass, "pass"),
            (Status::Fail, "fail"),
            (Status::Skip, "skip"),
            (Status::BlockedByProduct, "blocked-by-product"),
        ];
        for (status, spelling) in statuses {
            assert_eq!(status.as_str(), spelling);
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{spelling}\"")
            );
        }
    }

    #[test]
    fn a_case_carries_its_outcome_and_the_numbers_behind_it() {
        let mut case = CaseResult::new("download-100m");
        case.record("bytes", 104_857_600u64);
        case.record("throughput_mib_s", 91.5);
        case.record("hashes_agree", true);
        case.record("guest_sha256", "beef");
        let case = case.fail("the hashes differ");

        assert_eq!(case.status, Status::Fail);
        assert_eq!(case.error.as_deref(), Some("the hashes differ"));
        assert_eq!(case.measures["bytes"].to_string(), "104857600");
        assert_eq!(case.measures["throughput_mib_s"].to_string(), "91.500");
        assert_eq!(case.measures["hashes_agree"].to_string(), "true");
        assert_eq!(case.measures["guest_sha256"].to_string(), "beef");
    }

    #[test]
    fn a_skip_and_a_product_block_both_carry_a_reason() {
        let skipped = CaseResult::new("udp-echo").skip("no guest tool sends an empty datagram");
        let blocked = CaseResult::new("reset-mid-transfer").blocked("issue #427");

        assert_eq!(skipped.status, Status::Skip);
        assert_eq!(
            skipped.reason.as_deref(),
            Some("no guest tool sends an empty datagram")
        );
        assert_eq!(blocked.status, Status::BlockedByProduct);
        assert_eq!(blocked.reason.as_deref(), Some("issue #427"));
    }

    #[test]
    fn the_summary_counts_one_entry_per_status() {
        let mut result = minimal();
        result.cases = vec![
            CaseResult::new("a").pass(),
            CaseResult::new("b").pass(),
            CaseResult::new("c").fail("no"),
            CaseResult::new("d").skip("no host"),
        ];
        let counts = result.counts();

        assert_eq!(counts["pass"], 2);
        assert_eq!(counts["fail"], 1);
        assert_eq!(counts["skip"], 1);
        assert!(result.case("c").is_some());
        assert!(result.case("zz").is_none());
    }

    #[test]
    fn the_backend_names_the_cases_whose_difference_is_expected() {
        let result = minimal();
        assert!(result.expects_difference("lease-and-resolver"));
        assert!(!result.expects_difference("upload-100m"));
    }

    #[test]
    fn a_binary_is_recorded_by_the_hash_of_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lns");
        std::fs::write(&path, b"parity").unwrap();

        assert_eq!(
            sha256_file(&path).unwrap(),
            "65966f0faeeff2d783a9e9766d96bafb9ce7ea133ccabd263106f6d7ff1ddd14"
        );
    }

    #[test]
    fn the_clock_the_result_stamps_moves_forward() {
        assert!(unix_ms() > 1_700_000_000_000);
    }
}
