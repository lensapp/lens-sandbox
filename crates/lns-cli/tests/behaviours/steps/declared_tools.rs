use std::path::PathBuf;

use cucumber::{given, then};

use crate::world::BehaviourWorld;

#[given(regex = r#"^a lns\.yaml declaring tools \[(.*)\]$"#)]
fn lns_yaml_with_tools(w: &mut BehaviourWorld, list: String) {
    let entries: String = list
        .split(',')
        .map(|entry| format!("    - {}\n", entry.trim().trim_matches('"')))
        .collect();
    w.author_files.insert(
        PathBuf::from("/work/lns.yaml"),
        format!(
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  tools:\n{entries}"
        ),
    );
}

#[then("validation succeeds without touching the network or the service")]
fn validation_succeeds_offline(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code != 0 {
        return Err(format!(
            "expected exit code 0, got {} (output: {:?})",
            res.exit_code, res.output
        ));
    }
    if !res.output.contains("is valid") {
        return Err(format!("expected a validity verdict, got {:?}", res.output));
    }
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.is_empty() {
        Ok(())
    } else {
        Err(format!("expected no service request, saw {requests:?}"))
    }
}

#[then(regex = r#"^validation fails naming the entry and the expected "name@version" shape$"#)]
fn validation_fails_naming_shape(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 {
        return Err(format!("validation passed: {:?}", res.output));
    }
    if res.output.contains(r#""node@""#) && res.output.contains(r#""name@version""#) {
        Ok(())
    } else {
        Err(format!(
            "expected the entry and the \"name@version\" shape in {:?}",
            res.output
        ))
    }
}

#[then(
    regex = r#"^validation fails asking for an explicit version such as "node@22" or "node@latest"$"#
)]
fn validation_fails_asking_version(w: &mut BehaviourWorld) -> Result<(), String> {
    let res = w.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 {
        return Err(format!("validation passed: {:?}", res.output));
    }
    if res
        .output
        .contains(r#"explicit version such as "node@22" or "node@latest""#)
    {
        Ok(())
    } else {
        Err(format!(
            "expected the explicit-version ask in {:?}",
            res.output
        ))
    }
}
