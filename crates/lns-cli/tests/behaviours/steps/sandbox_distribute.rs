use cucumber::{given, then};
use lns_ipc::{Request, Response};

use crate::world::BehaviourWorld;

#[given("the registry accepts the push")]
fn registry_accepts_push(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Ok(format!("sha256:{}", "a".repeat(64))));
}

#[given(regex = r#"^the version index resolves "([^"]+)" to "([^"]+)"$"#)]
fn version_index_resolves(w: &mut BehaviourWorld, spec: String, exact: String) {
    w.tool_index.insert(spec, exact);
}

#[given(regex = r#"^the version index does not list "([^"]+)"$"#)]
fn version_index_does_not_list(w: &mut BehaviourWorld, spec: String) {
    w.unlisted_pins.insert(spec);
}

#[then("the published artifact carries the exact resolved versions")]
fn published_carries_exact_versions(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|e| format!("invalid pushed doc: {e}"))?;
    let tools = value["spec"]["tools"]
        .as_array()
        .ok_or("the pushed config carries no spec.tools")?;
    for (spec, exact) in &w.tool_index {
        let name = spec.split('@').next().unwrap_or_default();
        let pinned = format!("{name}@{exact}");
        if !tools.iter().any(|t| t.as_str() == Some(pinned.as_str())) {
            return Err(format!("expected {pinned:?} among {tools:?}"));
        }
        if tools.iter().any(|t| t.as_str() == Some(spec.as_str())) {
            return Err(format!("the fuzzy entry {spec:?} survived in {tools:?}"));
        }
    }
    Ok(())
}

#[given("the stored credential for the registry lacks push scope")]
fn credential_lacks_push_scope(w: &mut BehaviourWorld) {
    w.push_outcome = Some(Err("credential for ghcr.io lacks push scope".to_string()));
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn valid_lns_yaml_with_fileset(w: &mut BehaviourWorld, path: String, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - path: {path}\n      guestPath: {mount}\n"
        ),
    );
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring an inline fileset at \"([^\"]+)\"$"#
)]
fn valid_lns_yaml_with_inline_fileset(w: &mut BehaviourWorld, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - inline:\n        settings.json: do-not-print\n      guestPath: {mount}\n"
        ),
    );
}

#[then("the sandbox artifact carries the packed directory as a layer of its own")]
fn fileset_packed_into_the_artifact(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.pushed_layers.as_slice() {
        [digest] if digest.starts_with("sha256:") => Ok(()),
        other => Err(format!(
            "a fileset is not a separate artifact, so the directory has to travel as a layer of this one (docs/sandbox-spec.md §7); saw {other:?}"
        )),
    }
}

#[then("the artifact carries no packed layer")]
fn no_packed_layer(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.pushed_layers.is_empty() && w.pushed_doc.is_some() {
        Ok(())
    } else {
        Err(format!(
            "expected a config-only artifact, saw layers {:?} and a document={}",
            w.pushed_layers,
            w.pushed_doc.is_some()
        ))
    }
}

#[then("the published sandbox config carries the inline content unchanged")]
fn published_config_keeps_inline(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|error| format!("invalid pushed json: {error}"))?;
    let entry = &value["spec"]["filesets"][0];
    if entry["inline"]["settings.json"] == "do-not-print" && entry.get("path").is_none() {
        Ok(())
    } else {
        Err(format!("expected unchanged inline entry, got {entry}"))
    }
}

#[then("the published sandbox config keeps the fileset path it was authored with")]
fn published_config_keeps_the_fileset_path(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|e| format!("pushed doc is not json: {e}"))?;
    let entry = &value["spec"]["filesets"][0];
    if entry["path"] == "./skills" && entry["guestPath"] == "/root/.agent/skills" {
        Ok(())
    } else {
        Err(format!(
            "the published entry keeps its path and guestPath — the content is what moved into the artifact's digest (§6); got: {entry}"
        ))
    }
}

#[given(regex = r#"^the project file "([^"]+)" contains "([^"]+)"$"#)]
fn project_file_contains(w: &mut BehaviourWorld, path: String, content: String) {
    w.author_files
        .insert(std::path::PathBuf::from(format!("/work/{path}")), content);
}

#[given(regex = r#"^the project file "([^"]+)" is larger than the README limit$"#)]
fn project_file_over_readme_limit(w: &mut BehaviourWorld, path: String) {
    w.author_files.insert(
        std::path::PathBuf::from(format!("/work/{path}")),
        "x".repeat(lns_artifact::build::MAX_README_BYTES as usize + 1),
    );
}

#[then("the pushed artifact carries the README as a text/markdown layer")]
fn pushed_artifact_carries_readme(w: &mut BehaviourWorld) -> Result<(), String> {
    let (_, readme) = w.pushed_readmes.last().ok_or("nothing was pushed")?;
    match readme {
        Some((media_type, content))
            if media_type == lns_artifact::build::README_LAYER_MEDIA_TYPE
                && content == b"# hermes" =>
        {
            Ok(())
        }
        other => Err(format!(
            "the hub renders the README from a text/markdown layer of the artifact (docs/sandbox-spec.md §7.2); saw {other:?}"
        )),
    }
}

#[then("the pushed artifact carries no README layer")]
fn pushed_artifact_carries_no_readme(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.pushed_readmes.last() {
        Some((_, None)) => Ok(()),
        Some((reference, Some(_))) => Err(format!(
            "no README.md beside the document means no layer, but {reference} carries one"
        )),
        None => Err("nothing was pushed".into()),
    }
}

#[then(regex = r#"^the artifact pushed to "([^"]+)" carries a README layer$"#)]
fn artifact_pushed_to_carries_readme(
    w: &mut BehaviourWorld,
    repository: String,
) -> Result<(), String> {
    let (_, readme) = w
        .pushed_readmes
        .iter()
        .find(|(reference, _)| reference.starts_with(&format!("{repository}:")))
        .ok_or_else(|| format!("nothing published to {repository}; saw {:?}", w.pushed_refs))?;
    if readme.is_some() {
        Ok(())
    } else {
        Err(
            "each artifact ships the README beside its own document, so the mixin's README travels with the mixin, not the sandbox"
                .into(),
        )
    }
}

#[then("nothing is pushed")]
fn nothing_pushed(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.pushed_layers.is_empty() && w.pushed_doc.is_none() {
        Ok(())
    } else {
        Err(format!(
            "expected no uploads, saw layers {:?} and doc {:?}",
            w.pushed_layers,
            w.pushed_doc.is_some()
        ))
    }
}

#[given(regex = r#"^the sandbox "([^"]+)" is cached$"#)]
fn sandbox_is_cached(w: &mut BehaviourWorld, reference: String) {
    w.sandbox.response = Some(Response::ImageTagged {
        from: reference.clone(),
        to: reference,
    });
}

#[then(regex = r#"^the service received a request to pull "([^"]+)"$"#)]
fn service_received_pull(w: &mut BehaviourWorld, reference: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::PullImage { image, .. } if *image == reference))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a PullImage request for {reference:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the service was asked to tag "([^"]+)" as "([^"]+)"$"#)]
fn service_asked_to_tag(w: &mut BehaviourWorld, from: String, to: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::TagImage { from: f, to: t } if *f == from && *t == to))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a TagImage from {from:?} to {to:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the service was asked to remove the cached artifact "([^"]+)"$"#)]
fn service_asked_to_remove_artifact(w: &mut BehaviourWorld, image: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::RemoveImage { image: asked } if *asked == image))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a RemoveImage for {image:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the service was asked to inspect "([^"]+)"$"#)]
fn service_asked_to_inspect(w: &mut BehaviourWorld, image: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::InspectImage { image: asked, .. } if *asked == image))
    {
        Ok(())
    } else {
        Err(format!(
            "expected an InspectImage for {image:?} among {requests:?}"
        ))
    }
}

#[then(regex = r#"^the sandbox "([^"]+)" resolves to the same cached artifact$"#)]
fn sandbox_resolves_to_same_artifact(w: &mut BehaviourWorld, to: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::TagImage { to: t, .. } if *t == to))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a TagImage request tagging {to:?} among {requests:?}"
        ))
    }
}

#[given(
    regex = r#"^a valid lns\.yaml in the current directory declaring a hostPath fileset "([^"]+)" mounted at "([^"]+)"$"#
)]
fn valid_lns_yaml_with_host_path_fileset(w: &mut BehaviourWorld, source: String, mount: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  filesets:\n    - hostPath: {source}\n      guestPath: {mount}\n      optional: true\n"
        ),
    );
}

#[then("the published sandbox config carries the hostPath unchanged")]
fn published_config_keeps_host_path(w: &mut BehaviourWorld) -> Result<(), String> {
    let doc = w.pushed_doc.as_ref().ok_or("no definition was pushed")?;
    let value: serde_json::Value =
        serde_json::from_slice(doc).map_err(|error| format!("invalid pushed json: {error}"))?;
    let entry = &value["spec"]["filesets"][0];
    if entry["hostPath"] == "~/.gitconfig" && entry.get("path").is_none() {
        Ok(())
    } else {
        Err(format!(
            "a hostPath is what makes the artifact portable — packing or rewriting it would pin the author's machine, got {entry}"
        ))
    }
}

#[given(regex = r#"^an lns\.yaml layering on the local mixin "([^"]+)"$"#)]
fn lns_yaml_layering_on(w: &mut BehaviourWorld, path: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  mixins:\n    - {path}\n"
        ),
    );
}

#[given(regex = r#"^an lns\.yaml layering on the local mixins "([^"]+)" and "([^"]+)"$"#)]
fn lns_yaml_layering_on_two(w: &mut BehaviourWorld, first: String, second: String) {
    w.author_files.insert(
        std::path::PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  mixins:\n    - {first}\n    - {second}\n"
        ),
    );
}

#[given(regex = r#"^the local mixin at "([^"]+)" is named "([^"]+)"$"#)]
fn local_mixin_named(w: &mut BehaviourWorld, dir: String, name: String) {
    w.author_files.insert(
        std::path::PathBuf::from(format!(
            "/work/{}/lns.yaml",
            dir.trim_start_matches("./").trim_end_matches('/')
        )),
        format!(
            "apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  env:\n    MODE: research\n"
        ),
    );
}

#[given(regex = r#"^the local mixin at "([^"]+)" is named "([^"]+)" and layers on "([^"]+)"$"#)]
fn local_mixin_layering(w: &mut BehaviourWorld, dir: String, name: String, on: String) {
    w.author_files.insert(
        std::path::PathBuf::from(format!(
            "/work/{}/lns.yaml",
            dir.trim_start_matches("./").trim_end_matches('/')
        )),
        format!(
            "apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  mixins:\n    - {on}\n"
        ),
    );
}

#[given(regex = r#"^the registry accepts (\d+) upload\(s\) then refuses$"#)]
fn registry_accepts_then_refuses(w: &mut BehaviourWorld, count: usize) {
    w.push_outcome = Some(Ok(format!("sha256:{}", "a".repeat(64))));
    w.push_fails_after = Some(count);
}

#[then(regex = r#"^the published sandbox pins mixin "([^"]+)" by digest$"#)]
fn published_sandbox_pins_mixin(w: &mut BehaviourWorld, repository: String) {
    let doc = w
        .pushed_doc
        .clone()
        .expect("the sandbox config travels with the artifact");
    let value: serde_json::Value = serde_json::from_slice(&doc).expect("the config blob is json");
    let mixins = value["spec"]["mixins"]
        .as_array()
        .expect("the published document declares its mixins")
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect::<Vec<_>>();
    assert!(
        mixins
            .iter()
            .any(|entry| entry.starts_with(&format!("{repository}@sha256:"))),
        "a consumer has no copy of the author's directory, so the published entry has to be a digest in {repository}; got {mixins:?}"
    );
    assert!(
        !mixins.iter().any(|entry| entry.starts_with('.')),
        "no local path may reach the published bytes; got {mixins:?}"
    );
}

#[then(regex = r#"^the mixin "([^"]+)" was published before the sandbox$"#)]
fn mixin_published_before_sandbox(w: &mut BehaviourWorld, repository: String) {
    let position = w
        .pushed_refs
        .iter()
        .position(|reference| reference.starts_with(&format!("{repository}:")))
        .unwrap_or_else(|| {
            panic!(
                "the mixin has to publish under its own repository; uploads were {:?}",
                w.pushed_refs
            )
        });
    assert!(
        position + 1 < w.pushed_refs.len(),
        "a digest cannot be pinned before it exists, so the sandbox uploads after its mixins; uploads were {:?}",
        w.pushed_refs
    );
}

#[then(regex = r#"^the mixin "([^"]+)" was published under its own digest as a tag$"#)]
fn mixin_published_under_digest_tag(w: &mut BehaviourWorld, repository: String) {
    let reference = w
        .pushed_refs
        .iter()
        .find(|reference| reference.starts_with(&format!("{repository}:")))
        .unwrap_or_else(|| {
            panic!(
                "nothing published to {repository}; uploads were {:?}",
                w.pushed_refs
            )
        });
    let tag = reference
        .rsplit_once(':')
        .expect("the reference carries a tag")
        .1;
    assert!(
        tag.starts_with("sha256-") && tag.len() == "sha256-".len() + 64,
        "an untagged manifest can be pruned while a sandbox still pins it, and the tag is derived from the content so it cannot move; got {tag}"
    );
}

#[then(regex = r#"^exactly (\d+) artifact\(s\) were uploaded$"#)]
fn exactly_n_uploaded(w: &mut BehaviourWorld, count: usize) {
    assert_eq!(
        w.pushed_refs.len(),
        count,
        "uploads were {:?}",
        w.pushed_refs
    );
}

#[then("the published sandbox was not uploaded")]
fn sandbox_not_uploaded(w: &mut BehaviourWorld) {
    assert!(
        !w.pushed_refs
            .iter()
            .any(|reference| reference.contains("hermes")),
        "a sandbox that pins a digest which never landed would be unresolvable for every consumer; uploads were {:?}",
        w.pushed_refs
    );
}

#[given(regex = r#"^the local mixin at "([^"]+)" is named "([^"]+)" and declares tool "([^"]+)"$"#)]
fn local_mixin_declaring_tool(w: &mut BehaviourWorld, dir: String, name: String, tool: String) {
    w.author_files.insert(
        std::path::PathBuf::from(format!(
            "/work/{}/lns.yaml",
            dir.trim_start_matches("./").trim_end_matches('/')
        )),
        format!(
            "apiVersion: lns.run/v1\nkind: mixin\nname: {name}\nspec:\n  tools:\n    - {tool}\n"
        ),
    );
}
