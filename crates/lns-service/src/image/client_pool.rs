use std::collections::HashMap;
use std::sync::Mutex;

use oci_client::secrets::RegistryAuth;
use sha2::{Digest, Sha256};

pub(crate) struct ClientPool<C> {
    clients: Mutex<HashMap<String, C>>,
}

impl<C: Clone> ClientPool<C> {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, key: String, make: impl FnOnce() -> C) -> C {
        self.clients
            .lock()
            .expect("client pool mutex poisoned")
            .entry(key)
            .or_insert_with(make)
            .clone()
    }
}

/// A stable, secret-free identity for a credential so a re-login (new secret) keys a fresh client instead of reusing one that cached the old bearer token.
fn auth_fingerprint(auth: &RegistryAuth) -> String {
    match auth {
        RegistryAuth::Anonymous => "anon".to_string(),
        RegistryAuth::Basic(user, secret) => {
            let mut h = Sha256::new();
            h.update(user.as_bytes());
            h.update([0]);
            h.update(secret.as_bytes());
            format!("basic:{}", hex::encode(h.finalize()))
        }
        RegistryAuth::Bearer(token) => {
            format!("bearer:{}", hex::encode(Sha256::digest(token.as_bytes())))
        }
    }
}

pub(crate) fn pool_key(registry: &str, auth: &RegistryAuth) -> String {
    format!("{registry}\u{0}{}", auth_fingerprint(auth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn get_or_create_runs_the_factory_once_per_key() {
        let pool: ClientPool<u32> = ClientPool::new();
        let calls = AtomicU32::new(0);
        let a = pool.get_or_create("k".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            42
        });
        let b = pool.get_or_create("k".into(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            99
        });
        assert_eq!(a, 42);
        assert_eq!(b, 42, "a warm key returns the cached client, not a new one");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "factory runs only on the miss");
    }

    #[test]
    fn get_or_create_keeps_a_separate_client_per_key() {
        let pool: ClientPool<u32> = ClientPool::new();
        assert_eq!(pool.get_or_create("a".into(), || 1), 1);
        assert_eq!(pool.get_or_create("b".into(), || 2), 2);
        assert_eq!(pool.get_or_create("a".into(), || 3), 1, "key a still cached");
    }

    #[test]
    fn anonymous_fingerprint_is_stable_and_plain() {
        assert_eq!(auth_fingerprint(&RegistryAuth::Anonymous), "anon");
    }

    #[test]
    fn basic_fingerprint_hides_the_secret() {
        let secret = "hunter2-super-secret";
        let fp = auth_fingerprint(&RegistryAuth::Basic("alice".into(), secret.into()));
        assert!(fp.starts_with("basic:"));
        assert!(
            !fp.contains(secret),
            "the secret must never appear in the pool key: {fp}"
        );
    }

    #[test]
    fn basic_fingerprint_changes_with_user_and_with_secret() {
        let base = auth_fingerprint(&RegistryAuth::Basic("alice".into(), "s1".into()));
        let other_user = auth_fingerprint(&RegistryAuth::Basic("bob".into(), "s1".into()));
        let other_secret = auth_fingerprint(&RegistryAuth::Basic("alice".into(), "s2".into()));
        assert_ne!(base, other_user, "a different user re-keys the client");
        assert_ne!(base, other_secret, "a re-login with a new secret re-keys the client");
    }

    #[test]
    fn bearer_fingerprint_hides_the_token_and_differs_from_basic() {
        let token = "aaaa.bbbb.cccc";
        let fp = auth_fingerprint(&RegistryAuth::Bearer(token.into()));
        assert!(fp.starts_with("bearer:"));
        assert!(!fp.contains(token));
        assert_ne!(fp, auth_fingerprint(&RegistryAuth::Basic("x".into(), token.into())));
    }

    #[test]
    fn pool_key_separates_registries_and_credentials() {
        let anon = RegistryAuth::Anonymous;
        let cred = RegistryAuth::Basic("u".into(), "p".into());
        assert_ne!(
            pool_key("docker.io", &anon),
            pool_key("ghcr.io", &anon),
            "distinct registries never share a client"
        );
        assert_ne!(
            pool_key("docker.io", &anon),
            pool_key("docker.io", &cred),
            "distinct credentials on one registry never share a client"
        );
        assert_eq!(pool_key("docker.io", &anon), pool_key("docker.io", &RegistryAuth::Anonymous));
    }
}
