use cucumber::{given, then, when};

use crate::world::BehaviourWorld;

fn definition_with_tools(entries: &str) -> String {
    let tools: Vec<String> = entries
        .split(',')
        .map(|entry| format!("{:?}", entry.trim().trim_matches('"')))
        .collect();
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"registry.example.test/runtime:1","tools":[{}]}}}}"#,
        tools.join(",")
    )
}

fn launch(w: &mut BehaviourWorld) {
    let rig = w.tools.get_or_insert_with(Default::default);
    let definition = rig.definition.clone().expect("a definition is staged");
    let resolved = match lns_service::artifact::plan_local_sandbox(definition.as_bytes()) {
        Ok(resolved) => resolved,
        Err(e) => {
            rig.error = Some(format!("{e:#}"));
            return;
        }
    };
    let requests = match lns_artifact::tools::parse_all(&resolved.tools) {
        Ok(requests) => requests,
        Err(e) => {
            rig.error = Some(format!("{e:#}"));
            return;
        }
    };
    if let Err(refusal) = lns_service::tools::registry::refuse_unknown_tools(&requests) {
        rig.error = Some(refusal.to_string());
    }
}

#[given(regex = r#"^a lns\.yaml declaring tools \[(.*)\]$"#)]
fn lns_yaml_declaring_tools(w: &mut BehaviourWorld, entries: String) {
    let rig = w.tools.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_tools(&entries));
}

#[when("I run the sandbox")]
fn run_the_sandbox(w: &mut BehaviourWorld) {
    launch(w);
}

#[then("the launch is refused naming the unknown tool")]
fn refused_naming_the_unknown_tool(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.tools.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_ref().ok_or("the launch was not refused")?;
    if error.contains("definitely-not-a-tool") {
        Ok(())
    } else {
        Err(format!("expected the unknown tool named in: {error}"))
    }
}
