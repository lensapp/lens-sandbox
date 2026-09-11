use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

struct Mixins;
impl lns_service::artifact::mixin::MixinSource for Mixins {
    async fn fetch(
        &self,
        _: &lns_service::artifact::mixin::Locator,
    ) -> anyhow::Result<lns_service::artifact::mixin::FetchedMixin> {
        Ok(lns_service::artifact::mixin::FetchedMixin {
            pinned: "/tools/lns.yaml".into(),
            document: r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"tools","spec":{"tools":["node@22"],"egress":{"http":[{"match":"example.com","verdict":"allow"}],"tcp":[{"match":"db.example.com:5432","verdict":"allow"}]}}}"#.into(),
            layers: vec![],
        })
    }
}

#[when("a sandbox declaring Node 20 is previewed with a Node 22 mixin")]
async fn preview(world: &mut BehaviourWorld) {
    world.configuration = Some(lns_service::run::configuration::preview(
        br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"reviewer","spec":{"image":"alpine:3.20","tools":["node@20"]}}"#,
        &lns_service::artifact::mixin::Locator::Local("/project/lns.yaml".into()),
        &["/tools/lns.yaml".into()], &Mixins,
    ).await.unwrap());
}

#[then("the preview identifies the added mixin and the tool it replaces")]
fn previewed(world: &mut BehaviourWorld) {
    let config = world.configuration.as_ref().unwrap();
    assert!(
        config.document.contains("node@22"),
        "preview must show the resolved tool"
    );
    let sources = config.sources.as_ref().unwrap();
    assert_eq!(sources.added_mixins, ["/tools/lns.yaml"]);
    assert_eq!(config.rules.len(), 2);
    assert!(
        config
            .rules
            .iter()
            .all(|rule| rule.source == "/tools/lns.yaml")
    );
    assert!(
        sources
            .contributions
            .iter()
            .any(|c| c.source == "/tools/lns.yaml"
                && c.displaced.iter().any(|d| d.summary.contains("20")))
    );
}

// no-op: each inspection constructs the same in-memory recorded sandbox.
#[given("a recorded sandbox with document rules and a connector grant")]
fn recorded(_world: &mut BehaviourWorld) {}

fn inspect(decided: bool) -> lns_ipc::SandboxConfiguration {
    let record: lns_service::run_record::RunRecord = serde_json::from_value(serde_json::json!({
        "version": 1, "run_id": "run-1", "name": "reviewer", "args": {"cpus": 2, "mem": 1024, "cmd": [], "debug": false},
        "descriptor_sha256": "sha256:one", "layer_digests": [], "image": "alpine:3.20",
        "command": "sh", "created_at": "2026-09-11T12:00:00Z", "finished_at": null, "exit_code": null,
        "resolved_document": "{\"apiVersion\":\"lns.run/v1\",\"kind\":\"sandbox\",\"name\":\"reviewer\",\"spec\":{\"image\":\"alpine:3.20\",\"tools\":[\"node@22\"],\"egress\":{\"http\":[{\"match\":\"example.com\",\"verdict\":\"allow\"}]}}}"
    })).unwrap();
    let decisions: lns_policy::Policy = serde_yaml::from_str(if decided {
        "network:\n  egress:\n    http:\n      - match: example.com\n        verdict: deny\n"
    } else {
        "{}"
    })
    .unwrap();
    let grant = lns_service::approval_flow::protocol::GrantedPayload {
        egress: serde_yaml::from_str("network:\n  egress:\n    http:\n      - match: api.example.com\n        verdict: allow\n").unwrap(),
        env: [("TOKEN".into(), "must-not-leak".into())].into(),
        files: vec![lns_service::approval_flow::protocol::WireFile {
            path: "/home/agent/.gitconfig".into(),
            content: lns_service::approval_flow::protocol::WireFileContent::Content("must-not-leak".into()),
            mode: None, owner: None,
        }],
        ..Default::default()
    };
    lns_service::run::configuration::inspect(
        &record,
        &decisions,
        &[("github".into(), grant)].into(),
    )
    .unwrap()
}

#[when("the sandbox configuration is inspected after a deny decision")]
fn after_decision(world: &mut BehaviourWorld) {
    world.configuration = Some(inspect(true));
}

#[when("the sandbox configuration is inspected without a persistent decision")]
fn without_decision(world: &mut BehaviourWorld) {
    world.configuration = Some(inspect(false));
}

#[then("the configuration attributes the current rules in enforcement order")]
fn attributed(world: &mut BehaviourWorld) {
    let config = world.configuration.as_ref().unwrap();
    assert_eq!(
        config
            .rules
            .iter()
            .map(|r| r.source.as_str())
            .collect::<Vec<_>>(),
        [
            "Your decision",
            "Connector: github",
            "Definition and mixins"
        ]
    );
    assert!(config.rules[0].rule.contains("deny"));
    assert!(config.document.contains("node@22"));
}

#[then("the configuration exposes connector destinations but no secret values")]
fn no_secrets(world: &mut BehaviourWorld) {
    let config = world.configuration.as_ref().unwrap();
    assert_eq!(config.grants[0].variables, ["TOKEN"]);
    assert_eq!(config.grants[0].files, ["/home/agent/.gitconfig"]);
    assert!(
        !serde_json::to_string(config)
            .unwrap()
            .contains("must-not-leak")
    );
}

#[then("no user decision remains in the configuration")]
fn withdrawn(world: &mut BehaviourWorld) {
    let config = world.configuration.as_ref().unwrap();
    assert_eq!(config.rules.len(), 2);
    assert!(config.rules.iter().all(|r| r.source != "Your decision"));
}
