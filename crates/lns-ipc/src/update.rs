use serde::{Deserialize, Serialize};

pub const NO_UPDATE_CHECK_ENV: &str = "LNS_NO_UPDATE_CHECK";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub latest: String,
    pub min_secure_version: Option<String>,
    pub checked_at_unix: u64,
}
