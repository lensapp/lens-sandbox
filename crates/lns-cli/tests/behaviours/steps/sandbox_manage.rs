use cucumber::given;
use lns_ipc::{ImageInfo, Response};

use crate::world::BehaviourWorld;

#[given(regex = r#"^the service reports one cached sandbox "([^"]+)"$"#)]
fn reports_one_cached_sandbox(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageList {
        images: vec![ImageInfo {
            reference,
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 14 * 1024 * 1024,
            layers: 3,
            pulled: "2026-01-01T00:00:00Z".into(),
            in_use_by: None,
        }],
    });
}
