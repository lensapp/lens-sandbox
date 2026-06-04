use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

fn no_env(_: &str) -> Option<String> {
    None
}

fn env_with(key: &'static str) -> impl Fn(&str) -> Option<String> {
    move |k: &str| (k == key).then(|| "1".to_string())
}

struct FakeClock(u64);
impl Clock for FakeClock {
    fn now_unix(&self) -> u64 {
        self.0
    }
}

struct FakeStore {
    id: Mutex<Option<String>>,
    status: Mutex<Option<UpdateStatus>>,
    fail_read_id: bool,
    fail_write_id: bool,
    fail_write_status: bool,
}
impl Default for FakeStore {
    fn default() -> Self {
        FakeStore {
            id: Mutex::new(None),
            status: Mutex::new(None),
            fail_read_id: false,
            fail_write_id: false,
            fail_write_status: false,
        }
    }
}
impl StateStore for FakeStore {
    fn read_install_id(&self) -> Result<Option<String>> {
        if self.fail_read_id {
            bail!("read id");
        }
        Ok(self.id.lock().unwrap().clone())
    }
    fn write_install_id(&self, id: &str) -> Result<()> {
        if self.fail_write_id {
            bail!("write id");
        }
        *self.id.lock().unwrap() = Some(id.to_string());
        Ok(())
    }
    fn write_status(&self, status: &UpdateStatus) -> Result<()> {
        if self.fail_write_status {
            bail!("write status");
        }
        *self.status.lock().unwrap() = Some(status.clone());
        Ok(())
    }
}

struct FakeFetcher {
    body: Mutex<Option<Result<Vec<u8>>>>,
    called: AtomicBool,
    seen_ua: Mutex<Option<String>>,
    seen_id: Mutex<Option<String>>,
}
impl FakeFetcher {
    fn returning(body: Result<Vec<u8>>) -> Self {
        FakeFetcher {
            body: Mutex::new(Some(body)),
            called: AtomicBool::new(false),
            seen_ua: Mutex::new(None),
            seen_id: Mutex::new(None),
        }
    }
    fn ok(json: &str) -> Self {
        Self::returning(Ok(json.as_bytes().to_vec()))
    }
}
impl Fetcher for FakeFetcher {
    async fn get_manifest(
        &self,
        _url: &str,
        user_agent: &str,
        install_id: &str,
    ) -> Result<Vec<u8>> {
        self.called.store(true, Ordering::SeqCst);
        *self.seen_ua.lock().unwrap() = Some(user_agent.to_string());
        *self.seen_id.lock().unwrap() = Some(install_id.to_string());
        self.body.lock().unwrap().take().unwrap()
    }
}

const MANIFEST: &str = r#"{"version":"9.9.9","platforms":{}}"#;
const TEST_UA: &str = "lns/9.9.9 (os=Darwin; arch=arm64; kernel=Darwin/24.6.0; shell=unknown; method=service-update-check)";

#[tokio::test]
async fn disabled_via_env_does_nothing() {
    let store = FakeStore::default();
    let fetcher = FakeFetcher::ok(MANIFEST);
    check_once(
        env_with(DISABLE_ENV),
        &fetcher,
        &store,
        &FakeClock(1),
        CDN_BASE,
        TEST_UA,
    )
    .await
    .unwrap();
    assert!(
        !fetcher.called.load(Ordering::SeqCst),
        "no network when disabled"
    );
    assert!(
        store.id.lock().unwrap().is_none(),
        "no install ID generated when disabled"
    );
    assert!(store.status.lock().unwrap().is_none());
}

#[tokio::test]
async fn first_run_generates_id_and_marks_latest_version() {
    let store = FakeStore::default();
    let fetcher = FakeFetcher::ok(MANIFEST);
    check_once(
        no_env,
        &fetcher,
        &store,
        &FakeClock(1234),
        CDN_BASE,
        TEST_UA,
    )
    .await
    .unwrap();
    let id = store
        .id
        .lock()
        .unwrap()
        .clone()
        .expect("install ID persisted");
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(*fetcher.seen_id.lock().unwrap(), Some(id));
    assert_eq!(
        *store.status.lock().unwrap(),
        Some(UpdateStatus {
            latest: "9.9.9".to_string(),
            min_secure_version: None,
            checked_at_unix: 1234,
        })
    );
}

#[tokio::test]
async fn existing_id_is_reused_not_regenerated() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("existing-id".to_string());
    let fetcher = FakeFetcher::ok(MANIFEST);
    check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
        .await
        .unwrap();
    assert_eq!(
        *fetcher.seen_id.lock().unwrap(),
        Some("existing-id".to_string())
    );
    assert_eq!(*store.id.lock().unwrap(), Some("existing-id".to_string()));
}

#[tokio::test]
async fn check_marks_min_secure_version_when_present() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("id".to_string());
    let fetcher = FakeFetcher::ok(r#"{"version":"9.9.9","min_secure_version":"1.5.0"}"#);
    check_once(no_env, &fetcher, &store, &FakeClock(7), CDN_BASE, TEST_UA)
        .await
        .unwrap();
    assert_eq!(
        store
            .status
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .min_secure_version,
        Some("1.5.0".to_string())
    );
}

#[tokio::test]
async fn check_forwards_the_caller_supplied_user_agent_verbatim() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("id".to_string());
    let fetcher = FakeFetcher::ok(MANIFEST);
    check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
        .await
        .unwrap();
    assert_eq!(*fetcher.seen_ua.lock().unwrap(), Some(TEST_UA.to_string()));
}

#[tokio::test]
async fn offline_fetch_error_propagates_and_marks_nothing() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("id".to_string());
    let fetcher = FakeFetcher::returning(Err(anyhow::anyhow!("offline")));
    assert!(
        check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
            .await
            .is_err()
    );
    assert!(store.status.lock().unwrap().is_none());
}

#[tokio::test]
async fn garbage_manifest_propagates_error() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("id".to_string());
    let fetcher = FakeFetcher::ok("not json");
    assert!(
        check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn read_id_error_propagates() {
    let store = FakeStore {
        fail_read_id: true,
        ..Default::default()
    };
    let fetcher = FakeFetcher::ok(MANIFEST);
    assert!(
        check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn write_id_error_propagates() {
    let store = FakeStore {
        fail_write_id: true,
        ..Default::default()
    };
    let fetcher = FakeFetcher::ok(MANIFEST);
    assert!(
        check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn write_status_error_propagates() {
    let store = FakeStore {
        fail_write_status: true,
        ..Default::default()
    };
    *store.id.lock().unwrap() = Some("id".to_string());
    let fetcher = FakeFetcher::ok(MANIFEST);
    assert!(
        check_once(no_env, &fetcher, &store, &FakeClock(1), CDN_BASE, TEST_UA)
            .await
            .is_err()
    );
}

#[test]
fn ensure_install_id_generates_and_persists_when_absent() {
    let store = FakeStore::default();
    let id = ensure_install_id(&store).unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(*store.id.lock().unwrap(), Some(id));
}

#[test]
fn ensure_install_id_reuses_existing_without_regenerating() {
    let store = FakeStore::default();
    *store.id.lock().unwrap() = Some("existing-id".to_string());
    assert_eq!(ensure_install_id(&store).unwrap(), "existing-id");
}

#[test]
fn ensure_install_id_propagates_read_error() {
    let store = FakeStore {
        fail_read_id: true,
        ..Default::default()
    };
    assert!(ensure_install_id(&store).is_err());
}

#[test]
fn ensure_install_id_propagates_write_error() {
    let store = FakeStore {
        fail_write_id: true,
        ..Default::default()
    };
    assert!(ensure_install_id(&store).is_err());
}

#[test]
fn establish_install_id_writes_id_eagerly_when_enabled() {
    let store = FakeStore::default();
    establish_install_id(no_env, &store).unwrap();
    let id = store.id.lock().unwrap().clone().expect("id persisted");
    assert!(uuid::Uuid::parse_str(&id).is_ok());
}

#[test]
fn establish_install_id_is_a_no_op_when_opted_out() {
    let store = FakeStore::default();
    establish_install_id(env_with(DISABLE_ENV), &store).unwrap();
    assert!(
        store.id.lock().unwrap().is_none(),
        "no install ID established when opted out"
    );
}

#[test]
fn establish_install_id_propagates_store_error() {
    let store = FakeStore {
        fail_write_id: true,
        ..Default::default()
    };
    assert!(establish_install_id(no_env, &store).is_err());
}

#[test]
fn parse_manifest_reads_version_and_optional_min_secure() {
    let (v, m) = parse_manifest(MANIFEST.as_bytes()).unwrap();
    assert_eq!(v, "9.9.9");
    assert_eq!(m, None);
    let (_, m) = parse_manifest(br#"{"version":"1.0.0","min_secure_version":"0.9.0"}"#).unwrap();
    assert_eq!(m, Some("0.9.0".to_string()));
}

#[test]
fn parse_manifest_bails_on_missing_version() {
    assert!(parse_manifest(br#"{"version":""}"#).is_err());
    assert!(parse_manifest(b"not json").is_err());
}

#[test]
fn generated_install_id_is_random_uuid_v4() {
    let a = generate_install_id();
    let b = generate_install_id();
    assert_ne!(a, b);
    assert_eq!(
        uuid::Uuid::parse_str(&a).unwrap().get_version(),
        Some(uuid::Version::Random)
    );
}

#[test]
fn update_status_round_trips_through_serde() {
    let s = UpdateStatus {
        latest: "1.2.3".to_string(),
        min_secure_version: Some("1.0.0".to_string()),
        checked_at_unix: 42,
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: UpdateStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}
