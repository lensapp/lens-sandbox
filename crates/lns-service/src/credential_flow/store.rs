//! Credential store types live in `lns_policy::credentials` so `lns` CLI commands can read and write the credentials file without depending on lns-service; re-exported here to keep the in-crate path stable.

pub use lns_policy::credentials::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
    default_credentials_path,
};
