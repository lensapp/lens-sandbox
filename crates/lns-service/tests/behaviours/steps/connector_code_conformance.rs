use cucumber::{given, then};

use super::connector_code::{PLACEHOLDER, code_method, rig};
use crate::world::BehaviourWorld;

#[given(regex = r#"^its method "([^"]+)" uses the (token|code) mechanism$"#)]
fn its_method_uses_the_mechanism(w: &mut BehaviourWorld, method: String, mechanism: String) {
    if mechanism == "code" {
        code_method(
            w,
            &method,
            "granting",
            &["auth.some-provider.example"],
            false,
        );
        return;
    }
    rig(w).add_method(serde_json::json!({
        "name": method,
        "auth": { "kind": "token" },
        "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
        "credentials": [{ "envVar": "SOME_TOKEN", "placeholder": PLACEHOLDER, "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }] }],
    }));
    rig(w).install();
}

/// Each mechanism fails in its own way — a component says so, a pasted value is left empty — and what stands afterwards must not depend on which.
#[given(regex = r#"^the mechanism fails$"#)]
fn the_mechanism_fails(w: &mut BehaviourWorld) {
    if rig(w).carries_a_component() {
        rig(w).its_component_is("failing");
    } else {
        rig(w).answers_nothing();
    }
}

#[then(regex = r#"^the connection records the method "([^"]+)"$"#)]
fn the_connection_records_the_method(w: &mut BehaviourWorld, method: String) {
    let name = rig(w).last_declared_name().expect("a connector");
    let held = rig(w).connections_of(&name);
    assert_eq!(
        held.values().map(|c| c.method.as_str()).collect::<Vec<_>>(),
        [method.as_str()],
        "a connection names the method that produced it, whichever mechanism that was (§7.1)"
    );
}
