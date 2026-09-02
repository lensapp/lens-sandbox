use anyhow::{Context, Result};
use lns_ipc::SaveKind;
use lns_policy::Policy;
use serde_json::{Value, json};

use crate::run_record::RunRecord;

/// `docs/sandbox-spec.md` §8.4: a run written out as a document the developer keeps, named for the file it lands in.
pub(crate) fn render(
    record: &RunRecord,
    decisions: &Policy,
    kind: SaveKind,
    name: &str,
) -> Result<String> {
    match kind {
        SaveKind::Mixin => render_mixin(decisions, name),
        SaveKind::Sandbox => render_sandbox(record, decisions, name),
    }
}

fn render_mixin(decisions: &Policy, name: &str) -> Result<String> {
    let mut named = decisions.clone();
    named.name = Some(name.to_string());
    let bytes = named
        .document_bytes(std::path::Path::new(name))
        .context("rendering what this run decided")?;
    String::from_utf8(bytes).context("the rendered mixin is not utf-8")
}

fn render_sandbox(record: &RunRecord, decisions: &Policy, name: &str) -> Result<String> {
    let mut document = base_document(record)?;
    document["name"] = json!(name);
    let spec = document
        .get_mut("spec")
        .and_then(Value::as_object_mut)
        .context("the document this run booted has no spec")?;
    // Every source is already folded in, so a reference left here would resolve a second time on the next run.
    spec.remove("mixins");
    overlay_launch(spec, &record.args);
    root_declared_paths(spec, record.args.definition_dir.as_deref());
    spec.insert("egress".to_string(), effective_egress(spec, decisions)?);
    serde_yaml::to_string(&document).context("rendering this run as a document")
}

/// §8.4: a relative `source` means the directory of the document that declared it, and a saved document lands somewhere else, so every path it carries is written absolute.
fn root_declared_paths(spec: &mut serde_json::Map<String, Value>, project_dir: Option<&str>) {
    let Some(project_dir) = project_dir else {
        return;
    };
    let root = std::path::Path::new(project_dir);
    for (block, field) in [("volumes", "source"), ("filesets", "path")] {
        let Some(entries) = spec.get_mut(block).and_then(Value::as_array_mut) else {
            continue;
        };
        for entry in entries {
            let Some(declared) = entry.get(field).and_then(Value::as_str) else {
                continue;
            };
            let path = std::path::Path::new(declared);
            if path.is_absolute() {
                continue;
            }
            let absolute = lns_artifact::sandbox::fold_path(&root.join(path));
            entry[field] = json!(absolute.display().to_string());
        }
    }
}

/// The document the run booted, or the envelope a run with no document of its own needs before its launch is written into it.
fn base_document(record: &RunRecord) -> Result<Value> {
    match record.resolved_document.as_deref() {
        Some(json) => serde_json::from_str(json).context("parsing the document this run booted"),
        None => Ok(json!({
            "apiVersion": "lns.run/v1",
            "kind": "sandbox",
            "name": record.name,
            "spec": { "image": record.image },
        })),
    }
}

fn overlay_launch(spec: &mut serde_json::Map<String, Value>, args: &lns_ipc::RunImageArgs) {
    if !args.cmd.is_empty() {
        spec.insert("command".to_string(), json!(args.cmd.join(" ")));
    }
    set_if_declared(spec, "workdir", args.workdir.as_deref());
    set_if_declared(spec, "user", args.sandbox_user.as_deref());
    set_if_declared(spec, "entrypoint", args.entrypoint.as_deref());
    overlay_env(spec, &args.env);
    overlay_mounts(spec, args);
    if !args.published_ports.is_empty() {
        spec.insert("ports".to_string(), ports_of(&args.published_ports));
    }
    spec.insert(
        "resources".to_string(),
        json!({ "cpu": args.cpus, "memory": format!("{}Mi", args.mem) }),
    );
}

/// The run's mounts reach the service already merged with the document's, in a shape that carries less — no `exclude`, no `size`. So a guest path the document declares keeps the document's entry, and only one it does not declare is added from the run.
fn overlay_mounts(spec: &mut serde_json::Map<String, Value>, args: &lns_ipc::RunImageArgs) {
    let mut mounts: Vec<Value> = spec
        .get("volumes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let declared: Vec<String> = mounts
        .iter()
        .filter_map(|entry| entry.get("target").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let from_the_run = args
        .binds
        .iter()
        .map(|bind| {
            (
                bind.target.clone(),
                json!({
                    "type": "bind",
                    "source": bind.host_source,
                    "target": bind.target,
                    "readOnly": bind.read_only,
                }),
            )
        })
        .chain(args.volumes.iter().map(|volume| {
            (
                volume.target.clone(),
                json!({
                    "name": volume.name,
                    "target": volume.target,
                    "readOnly": volume.read_only,
                }),
            )
        }))
        .filter(|(target, _)| !declared.contains(target))
        .map(|(_, entry)| entry);
    mounts.extend(from_the_run);
    if !mounts.is_empty() {
        spec.insert("volumes".to_string(), Value::Array(mounts));
    }
}

/// A field the run did not set stays as the document wrote it, since writing `null` would override the image's own value.
fn set_if_declared(spec: &mut serde_json::Map<String, Value>, field: &str, value: Option<&str>) {
    if let Some(value) = value {
        spec.insert(field.to_string(), json!(value));
    }
}

/// A per-run `-e` outranks what the document declared, so it lands over the declared map rather than beside it.
fn overlay_env(spec: &mut serde_json::Map<String, Value>, env: &[String]) {
    let mut declared = spec
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for entry in env {
        if let Some((key, value)) = entry.split_once('=') {
            declared.insert(key.to_string(), json!(value));
        }
    }
    if !declared.is_empty() {
        spec.insert("env".to_string(), Value::Object(declared));
    }
}

fn ports_of(published: &[lns_ipc::PortPublish]) -> Value {
    Value::Array(
        published
            .iter()
            .map(|p| json!({ "host": p.host_port, "container": p.container_port }))
            .collect(),
    )
}

/// §8.1 puts the run's decisions after every document, so what it decided leads the table the saved document carries.
fn effective_egress(spec: &serde_json::Map<String, Value>, decisions: &Policy) -> Result<Value> {
    let authored: lns_policy::Egress = match spec.get("egress") {
        Some(value) => {
            serde_json::from_value(value.clone()).context("reading the egress this run resolved")?
        }
        None => lns_policy::Egress::default(),
    };
    let baseline = Policy {
        network: lns_policy::NetworkPolicy { egress: authored },
        name: None,
        rest: Default::default(),
    };
    let effective = crate::artifact::policy::merge_effective(Some(&baseline), None, decisions);
    serde_json::to_value(effective.network.egress).context("rendering the egress this run enforced")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_with(document: Option<&str>) -> RunRecord {
        let mut record = crate::run_record::test_record("aa01");
        record.name = "brave_lion".to_string();
        record.image = "docker.io/library/alpine:3.20".to_string();
        record.resolved_document = document.map(str::to_string);
        record
    }

    fn decided(matches: &str) -> Policy {
        let mut policy = Policy::default();
        policy.add_rule(lns_policy::RouteRule {
            match_pattern: matches.to_string(),
            verdict: lns_policy::Verdict::Allow,
            transport: lns_policy::Transport::default(),
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        });
        policy
    }

    fn yaml(text: &str) -> serde_yaml::Value {
        serde_yaml::from_str(text).expect("a document the user can validate")
    }

    #[test]
    fn a_saved_mixin_holds_what_the_run_decided_and_nothing_else() {
        let text = render(
            &record_with(None),
            &decided("git.example.test"),
            SaveKind::Mixin,
            "agreed-egress",
        )
        .expect("a run that decided something can be written out");
        let doc = yaml(&text);
        assert_eq!(
            doc["kind"], "mixin",
            "§8.4 writes what the run decided as a mixin; got: {text}"
        );
        assert_eq!(
            doc["name"], "agreed-egress",
            "§8.4 names the document for the file it lands in; got: {text}"
        );
        assert_eq!(
            doc["spec"]["egress"]["http"][0]["match"], "git.example.test",
            "the decision is the whole point of saving it; got: {text}"
        );
        assert!(
            doc["spec"].get("image").is_none(),
            "a mixin that carried the run's image would relaunch it; got: {text}"
        );
    }

    #[test]
    fn a_saved_sandbox_carries_the_egress_the_run_enforced() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","egress":{"http":[{"match":"docs.example.test","verdict":"allow"}],"tcp":[]}}}"#;
        let text = render(
            &record_with(Some(document)),
            &decided("git.example.test"),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect("a run with a document can be written out");
        let doc = yaml(&text);
        let http = doc["spec"]["egress"]["http"]
            .as_sequence()
            .expect("an http table")
            .iter()
            .map(|e| e["match"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert!(
            http.contains(&"docs.example.test".to_string())
                && http.contains(&"git.example.test".to_string()),
            "§8.4 saves the whole egress the run enforced, its own decisions included; got: {http:?}"
        );
        assert_eq!(
            http.first().map(String::as_str),
            Some("git.example.test"),
            "§8.1 puts the run's decisions last in the merge, so §4.2 places them ahead in the table; got: {http:?}"
        );
    }

    #[test]
    fn a_saved_sandbox_writes_a_relative_source_as_the_path_the_run_resolved() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","volumes":[{"type":"bind","source":"./src","target":"/work"},{"type":"bind","source":"/etc/ssl","target":"/ssl"}],"filesets":[{"path":"./skills","guestPath":"/skills"}]}}"#;
        let mut record = record_with(Some(document));
        record.args.definition_dir = Some("/home/dev/my-app".into());
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run with relative sources can be written out");
        let doc = yaml(&text);
        assert_eq!(
            doc["spec"]["volumes"][0]["source"], "/home/dev/my-app/src",
            "§8.4 saves the path the run resolved, or the saved document binds whatever sits beside it; got: {text}"
        );
        assert_eq!(
            doc["spec"]["volumes"][1]["source"], "/etc/ssl",
            "an absolute source already means one thing everywhere; got: {text}"
        );
        assert_eq!(
            doc["spec"]["filesets"][0]["path"], "/home/dev/my-app/skills",
            "a fileset roots the same way a bind does; got: {text}"
        );
    }

    #[test]
    fn a_named_volume_keeps_its_name_and_a_document_with_no_filesets_is_left_alone() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","volumes":[{"name":"cache","target":"/cache"}]}}"#;
        let mut record = record_with(Some(document));
        record.args.definition_dir = Some("/home/dev/my-app".into());
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run mounting a named volume can be written out");
        let doc = yaml(&text);
        assert_eq!(
            doc["spec"]["volumes"][0]["name"], "cache",
            "a named volume's source is its name, and rooting it would ask for a volume nobody created; got: {text}"
        );
        assert!(
            doc["spec"]["volumes"][0].get("source").is_none(),
            "rooting must not invent a source the document never declared; got: {text}"
        );
        assert!(
            doc["spec"].get("filesets").is_none(),
            "a document declaring no filesets gains none by being saved; got: {text}"
        );
    }

    #[test]
    fn a_run_from_a_published_reference_has_no_project_directory_to_root_against() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","volumes":[{"name":"cache","target":"/cache"}]}}"#;
        let mut record = record_with(Some(document));
        record.args.definition_dir = None;
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a pulled run can be written out");
        assert_eq!(
            yaml(&text)["spec"]["volumes"][0]["name"],
            "cache",
            "a named volume has no path to root, and inventing one would ask for a volume nobody created; got: {text}"
        );
    }

    /// §8.4 keeps a grant out of a saved `sandbox`, so the saved egress is exactly the documents' plus this run's own answers.
    #[test]
    fn a_saved_egress_is_the_documents_plus_this_runs_answers_and_nothing_else() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","egress":{"http":[{"match":"docs.example.test","verdict":"allow"}],"tcp":[]}}}"#;
        let text = render(
            &record_with(Some(document)),
            &decided("git.example.test"),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect("a run that granted a connector can still be written out");
        let doc = yaml(&text);
        let http: Vec<String> = doc["spec"]["egress"]["http"]
            .as_sequence()
            .expect("an http table")
            .iter()
            .map(|e| e["match"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            http,
            ["git.example.test", "docs.example.test"],
            "the saved egress is the documents' plus this run's own answers, and nothing a connector opened; got: {text}"
        );
        assert!(
            doc["spec"].get("credentials").is_none(),
            "a granted method's credentials are consent too, and carrying them would leak the grant a different way; got: {text}"
        );
    }

    #[test]
    fn a_saved_sandbox_drops_the_mixins_it_already_merged() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","mixins":["ghcr.io/team/tools@sha256:aa"]}}"#;
        let text = render(
            &record_with(Some(document)),
            &Policy::default(),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect("a run that merged a mixin can be written out");
        assert!(
            yaml(&text)["spec"].get("mixins").is_none(),
            "the mixin is folded in already, and resolving it again could bring different bytes; got: {text}"
        );
    }

    #[test]
    fn a_run_from_a_plain_image_saves_as_a_document_naming_that_image() {
        let text = render(
            &record_with(None),
            &Policy::default(),
            SaveKind::Sandbox,
            "scratch",
        )
        .expect("a run with no document of its own still ran something");
        let doc = yaml(&text);
        assert_eq!(doc["kind"], "sandbox");
        assert_eq!(
            doc["spec"]["image"], "docker.io/library/alpine:3.20",
            "the image it booted is what makes the saved document runnable; got: {text}"
        );
    }

    #[test]
    fn a_saved_sandbox_records_the_size_the_run_booted_with() {
        let mut record = record_with(None);
        record.args.cpus = 4;
        record.args.mem = 2048;
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "big")
            .expect("a resized run can be written out");
        let doc = yaml(&text);
        assert_eq!(doc["spec"]["resources"]["cpu"], 4);
        assert_eq!(
            doc["spec"]["resources"]["memory"], "2048Mi",
            "a size saved without its unit would read as bytes; got: {text}"
        );
    }

    #[test]
    fn a_per_run_variable_outranks_the_one_the_document_declared() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","env":{"MODE":"dev","KEEP":"yes"}}}"#;
        let mut record = record_with(Some(document));
        record.args.env = vec!["MODE=prod".to_string()];
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run with an overridden variable can be written out");
        let doc = yaml(&text);
        assert_eq!(
            doc["spec"]["env"]["MODE"], "prod",
            "the run booted with the flag's value, so saving the declared one would not reproduce it; got: {text}"
        );
        assert_eq!(
            doc["spec"]["env"]["KEEP"], "yes",
            "a flag replaces one variable, not the whole map; got: {text}"
        );
    }

    #[test]
    fn a_saved_sandbox_carries_the_mounts_the_run_was_started_with() {
        let mut record = record_with(None);
        record.args.binds = vec![lns_ipc::BindMount {
            host_source: "/home/dev/my-app/src".into(),
            target: "/work".into(),
            read_only: false,
            dropped_paths: Vec::new(),
            kept_paths: Vec::new(),
            excluded_paths: Vec::new(),
        }];
        record.args.volumes = vec![lns_ipc::VolumeMount {
            name: "cache".into(),
            target: "/cache".into(),
            read_only: true,
            size_bytes: None,
        }];
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run with mounts can be written out");
        let doc = yaml(&text);
        let mounts = doc["spec"]["volumes"]
            .as_sequence()
            .expect("a volumes list");
        assert_eq!(
            mounts[0]["source"], "/home/dev/my-app/src",
            "a saved document with no bind does not reproduce the run it claims to be; got: {text}"
        );
        assert_eq!(mounts[0]["target"], "/work");
        assert_eq!(
            mounts[1]["name"], "cache",
            "a named volume is named, not sourced; got: {text}"
        );
        assert_eq!(mounts[1]["readOnly"], true);
    }

    /// The wire shape carries no `exclude` and no `size`, so a declared target that took it would lose an isolation control and a capacity floor with nothing in the file to say a rule was ever there.
    #[test]
    fn a_declared_mount_is_saved_as_the_document_wrote_it() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","volumes":[{"type":"bind","source":"./","target":"/work","exclude":[".cargo","node_modules"]},{"name":"cache","target":"/cache","size":"100Gi"}]}}"#;
        let mut record = record_with(Some(document));
        record.args.definition_dir = Some("/home/dev/my-app".into());
        record.args.binds = vec![lns_ipc::BindMount {
            host_source: "/home/dev/my-app".into(),
            target: "/work".into(),
            read_only: false,
            dropped_paths: Vec::new(),
            kept_paths: Vec::new(),
            excluded_paths: Vec::new(),
        }];
        record.args.volumes = vec![lns_ipc::VolumeMount {
            name: "cache".into(),
            target: "/cache".into(),
            read_only: false,
            size_bytes: None,
        }];
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run whose mounts the document declared can be written out");
        let doc = yaml(&text);
        let mounts = doc["spec"]["volumes"]
            .as_sequence()
            .expect("a volumes list");
        assert_eq!(
            mounts.len(),
            2,
            "the run's copy of a declared mount is the same mount, not a second one; got: {text}"
        );
        assert_eq!(
            mounts[0]["exclude"][0], ".cargo",
            "§3.1.10 keeps an excluded subpath out of the guest, and a saved document that drops it mounts what the author hid; got: {text}"
        );
        assert_eq!(
            mounts[0]["source"], "/home/dev/my-app",
            "a relative source still roots, so the saved document means the same thing anywhere; got: {text}"
        );
        assert_eq!(
            mounts[1]["size"], "100Gi",
            "a size is a floor, and dropping it silently resizes the volume to the default; got: {text}"
        );
    }

    #[test]
    fn a_mount_the_run_added_that_no_document_declared_is_saved_with_it() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","volumes":[{"name":"kept","target":"/other"}]}}"#;
        let mut record = record_with(Some(document));
        record.args.definition_dir = Some("/home/dev/my-app".into());
        record.args.binds = vec![lns_ipc::BindMount {
            host_source: "/home/dev/my-app/src".into(),
            target: "/work".into(),
            read_only: false,
            dropped_paths: Vec::new(),
            kept_paths: Vec::new(),
            excluded_paths: Vec::new(),
        }];
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run with an extra -v can be written out");
        let doc = yaml(&text);
        let mounts = doc["spec"]["volumes"]
            .as_sequence()
            .expect("a volumes list");
        assert_eq!(mounts.len(), 2, "got: {text}");
        assert_eq!(
            mounts[0]["name"], "kept",
            "a declared mount the run did not touch survives; got: {text}"
        );
        assert_eq!(
            mounts[1]["source"], "/home/dev/my-app/src",
            "a `-v` no document declared is part of what the run was; got: {text}"
        );
    }

    #[test]
    fn a_saved_sandbox_publishes_the_ports_the_run_published() {
        let mut record = record_with(None);
        record.args.published_ports = vec![lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::from([127, 0, 0, 1]),
            host_port: 8080,
            container_port: 80,
            protocol: lns_ipc::Protocol::Tcp,
        }];
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "web")
            .expect("a run publishing a port can be written out");
        let doc = yaml(&text);
        assert_eq!(doc["spec"]["ports"][0]["host"], 8080);
        assert_eq!(doc["spec"]["ports"][0]["container"], 80);
    }

    #[test]
    fn a_document_that_does_not_parse_names_itself_rather_than_writing_half_a_file() {
        let err = render(
            &record_with(Some("not a document")),
            &Policy::default(),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect_err("a damaged record must not produce a document the user trusts");
        assert!(
            format!("{err:#}").contains("the document this run booted"),
            "the message has to say which of the two inputs failed; got: {err:#}"
        );
    }

    #[test]
    fn a_document_with_no_spec_is_refused() {
        let err = render(
            &record_with(Some(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"x"}"#,
            )),
            &Policy::default(),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect_err("a document with no spec describes no sandbox");
        assert!(format!("{err:#}").contains("no spec"), "got: {err:#}");
    }

    #[test]
    fn an_egress_block_that_does_not_parse_names_itself() {
        let err = render(
            &record_with(Some(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"x","spec":{"image":"a:1","egress":7}}"#,
            )),
            &Policy::default(),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect_err("an egress block that is not a table decides nothing");
        assert!(
            format!("{err:#}").contains("the egress this run resolved"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_saved_sandbox_records_the_identity_and_entrypoint_the_run_booted_with() {
        let mut record = record_with(None);
        record.args.workdir = Some("/workspace".into());
        record.args.sandbox_user = Some("agent".into());
        record.args.entrypoint = Some("/bin/sh".into());
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "reviewer")
            .expect("a run with per-run overrides can be written out");
        let doc = yaml(&text);
        assert_eq!(doc["spec"]["workdir"], "/workspace");
        assert_eq!(doc["spec"]["user"], "agent");
        assert_eq!(
            doc["spec"]["entrypoint"], "/bin/sh",
            "a saved document that dropped the entrypoint would run something else; got: {text}"
        );
    }

    #[test]
    fn a_run_that_overrode_nothing_leaves_those_fields_as_the_document_wrote_them() {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"upstream","spec":{"image":"alpine:3.20","workdir":"/srv"}}"#;
        let text = render(
            &record_with(Some(document)),
            &Policy::default(),
            SaveKind::Sandbox,
            "reviewer",
        )
        .expect("a run with no overrides can be written out");
        let doc = yaml(&text);
        assert_eq!(
            doc["spec"]["workdir"], "/srv",
            "an unset flag is not an instruction to clear what the document declared; got: {text}"
        );
        assert!(doc["spec"].get("user").is_none(), "got: {text}");
    }

    #[test]
    fn a_run_that_kept_the_images_command_saves_no_command_of_its_own() {
        let mut record = record_with(None);
        record.args.cmd = Vec::new();
        let text = render(&record, &Policy::default(), SaveKind::Sandbox, "scratch")
            .expect("a run with no command can be written out");
        assert!(
            yaml(&text)["spec"].get("command").is_none(),
            "an empty command written down would override the image's own; got: {text}"
        );
    }
}
