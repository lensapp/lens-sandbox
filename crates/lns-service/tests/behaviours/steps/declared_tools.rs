use cucumber::{given, then, when};
use lns_service::tools::{Arch, Libc, ProvisionTarget};

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

async fn launch(w: &mut BehaviourWorld) {
    let rig = w.tools.get_or_insert_with(Default::default);
    rig.error = None;
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
        return;
    }
    let target = ProvisionTarget {
        arch: Arch::Aarch64,
        libc: Libc::Gnu,
    };
    match lns_service::tools::ensure_tools(
        &rig.records,
        &rig.cache,
        &rig.provisioner,
        &requests,
        &target,
        "2026.7.14",
        1_700_000_000,
    )
    .await
    {
        Ok(ensured) => rig.ensured = Some(ensured),
        Err(e) => rig.error = Some(e.to_string()),
    }
}

#[given(regex = r#"^a lns\.yaml declaring tools \[(.*)\]$"#)]
fn lns_yaml_declaring_tools(w: &mut BehaviourWorld, entries: String) {
    let rig = w.tools.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_tools(&entries));
}

#[given(regex = r#"^tools \[(.*)\] were provisioned by an earlier run on this machine$"#)]
async fn tools_provisioned_earlier(w: &mut BehaviourWorld, entries: String) {
    let rig = w.tools.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_tools(&entries));
    launch(w).await;
    let rig = w.tools.as_ref().expect("rig");
    assert!(
        rig.error.is_none(),
        "the earlier run failed: {:?}",
        rig.error
    );
    assert_eq!(rig.provisioner.calls.lock().unwrap().len(), 1);
}

#[given("a lns.yaml declaring a tool whose download cannot complete")]
fn tool_download_cannot_complete(w: &mut BehaviourWorld) {
    let rig = w.tools.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_tools(r#""node@22""#));
    *rig.provisioner.fail_next.lock().unwrap() =
        Some("fetching https://nodejs.org/dist/: connection timed out".into());
}

#[when("I run the sandbox")]
async fn run_the_sandbox(w: &mut BehaviourWorld) {
    launch(w).await;
}

#[when("I run the sandbox for the first time")]
async fn run_the_sandbox_first_time(w: &mut BehaviourWorld) {
    launch(w).await;
}

#[when("I run the sandbox again")]
async fn run_the_sandbox_again(w: &mut BehaviourWorld) {
    launch(w).await;
}

#[then("the run starts without downloading anything")]
fn run_starts_without_downloads(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.tools.as_ref().ok_or("no launch happened")?;
    if let Some(error) = &rig.error {
        return Err(format!("the run failed: {error}"));
    }
    if rig.ensured.is_none() {
        return Err("the run composed no tools".into());
    }
    let calls = rig.provisioner.calls.lock().unwrap();
    if calls.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected no provisioning beyond the earlier run, saw {} calls",
            calls.len()
        ))
    }
}

#[then("the launch is refused naming the tool and the cause")]
fn refused_naming_tool_and_cause(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.tools.as_ref().ok_or("no launch happened")?;
    let error = rig.error.as_ref().ok_or("the launch was not refused")?;
    if error.contains("node@22") && error.contains("connection timed out") {
        Ok(())
    } else {
        Err(format!("expected the tool and cause in: {error}"))
    }
}

#[then("a later run retries from a clean state")]
async fn later_run_retries_clean(w: &mut BehaviourWorld) -> Result<(), String> {
    {
        let rig = w.tools.as_ref().ok_or("no launch happened")?;
        if !rig.cache.map.lock().unwrap().is_empty() {
            return Err("the failed provision left a cache entry".into());
        }
        if rig.records.record.lock().unwrap().is_some() {
            return Err("the failed provision recorded a resolution".into());
        }
    }
    launch(w).await;
    let rig = w.tools.as_ref().ok_or("no relaunch happened")?;
    match (&rig.error, &rig.ensured) {
        (None, Some(_)) => Ok(()),
        (error, _) => Err(format!("the retry did not succeed: {error:?}")),
    }
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

#[then("the resolved exact version is recorded on this machine")]
fn resolved_version_recorded(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.tools.as_ref().ok_or("no launch happened")?;
    let record = rig.records.record.lock().unwrap();
    let record = record.as_ref().ok_or("nothing was recorded")?;
    let entry = record
        .recorded("node@22")
        .ok_or("node@22 has no recorded resolution")?;
    if entry.resolved == "22.11.0" {
        Ok(())
    } else {
        Err(format!("expected 22.11.0, got {}", entry.resolved))
    }
}

#[then("later runs here use the recorded version even after upstream releases a newer 22.x")]
async fn later_runs_use_the_record(w: &mut BehaviourWorld) -> Result<(), String> {
    {
        let rig = w.tools.as_mut().ok_or("no launch happened")?;
        *rig.provisioner.upstream_patch.lock().unwrap() = "12.0".into();
        rig.cache.map.lock().unwrap().clear();
    }
    launch(w).await;
    let rig = w.tools.as_ref().ok_or("no relaunch happened")?;
    if let Some(error) = &rig.error {
        return Err(format!("the later run failed: {error}"));
    }
    let calls = rig.provisioner.calls.lock().unwrap();
    let asked = calls
        .last()
        .and_then(|call| call.first())
        .ok_or("no call")?;
    if asked.version != "22.11.0" {
        return Err(format!(
            "expected the recorded exact 22.11.0 to be requested, got {}",
            asked.version
        ));
    }
    let record = rig.records.record.lock().unwrap();
    let entry = record
        .as_ref()
        .and_then(|record| record.recorded("node@22").cloned())
        .ok_or("the record lost node@22")?;
    if entry.resolved == "22.11.0" {
        Ok(())
    } else {
        Err(format!("the record drifted to {}", entry.resolved))
    }
}
