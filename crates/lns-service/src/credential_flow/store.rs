//! Credential store types live in `lns_policy::credentials` so the storage backends stay reusable outside lns-service; re-exported here to keep the in-crate path stable.

pub use lns_policy::credentials::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
    default_credentials_path,
};
