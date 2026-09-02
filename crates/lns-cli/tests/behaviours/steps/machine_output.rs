use crate::world::BehaviourWorld;
use cucumber::then;

fn parsed(world: &BehaviourWorld) -> Result<serde_json::Value, String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    serde_json::from_str(&res.output)
        .map_err(|e| format!("output is not json ({e}): {:?}", res.output))
}

fn row(world: &BehaviourWorld, index: usize) -> Result<serde_json::Value, String> {
    let doc = parsed(world)?;
    let rows = doc
        .as_array()
        .ok_or_else(|| format!("output is not a json array: {doc}"))?;
    rows.get(index)
        .cloned()
        .ok_or_else(|| format!("no row {index} in {doc}"))
}

/// Reads a dotted path so a scenario can pin a nested key such as `status.state`.
fn field(value: &serde_json::Value, key: &str) -> Result<serde_json::Value, String> {
    let mut current = value;
    for segment in key.split('.') {
        current = current
            .get(segment)
            .ok_or_else(|| format!("no key {segment:?} in {current}"))?;
    }
    Ok(current.clone())
}

#[then(regex = r"^the output is a JSON array of (\d+) rows$")]
fn output_is_json_array_of(world: &mut BehaviourWorld, expected: usize) -> Result<(), String> {
    let doc = parsed(world)?;
    let rows = doc
        .as_array()
        .ok_or_else(|| format!("output is not a json array: {doc}"))?;
    if rows.len() == expected {
        Ok(())
    } else {
        Err(format!(
            "expected {expected} rows, got {}: {doc}",
            rows.len()
        ))
    }
}

#[then(regex = r"^the output is an empty JSON array$")]
fn output_is_empty_json_array(world: &mut BehaviourWorld) -> Result<(), String> {
    let doc = parsed(world)?;
    if doc == serde_json::json!([]) {
        Ok(())
    } else {
        Err(format!("expected an empty json array, got {doc}"))
    }
}

#[then(regex = r"^the output is a JSON object$")]
fn output_is_json_object(world: &mut BehaviourWorld) -> Result<(), String> {
    let doc = parsed(world)?;
    if doc.is_object() {
        Ok(())
    } else {
        Err(format!("expected a single json object, got {doc}"))
    }
}

#[then(regex = r"^the output is JSON null$")]
fn output_is_json_null(world: &mut BehaviourWorld) -> Result<(), String> {
    let doc = parsed(world)?;
    if doc.is_null() {
        Ok(())
    } else {
        Err(format!("expected json null, got {doc}"))
    }
}

#[then(regex = r#"^the JSON object has "([^"]+)" set to "([^"]*)"$"#)]
fn json_object_string(
    world: &mut BehaviourWorld,
    key: String,
    expected: String,
) -> Result<(), String> {
    let found = field(&parsed(world)?, &key)?;
    if found == serde_json::Value::String(expected.clone()) {
        Ok(())
    } else {
        Err(format!("expected {key} to be {expected:?}, got {found}"))
    }
}

#[then(regex = r#"^JSON row (\d+) has "([^"]+)" set to "([^"]*)"$"#)]
fn json_row_string(
    world: &mut BehaviourWorld,
    index: usize,
    key: String,
    expected: String,
) -> Result<(), String> {
    let found = field(&row(world, index)?, &key)?;
    if found == serde_json::Value::String(expected.clone()) {
        Ok(())
    } else {
        Err(format!("expected {key} to be {expected:?}, got {found}"))
    }
}

#[then(regex = r#"^JSON row (\d+) has a null "([^"]+)"$"#)]
fn json_row_null(world: &mut BehaviourWorld, index: usize, key: String) -> Result<(), String> {
    let found = field(&row(world, index)?, &key)?;
    if found.is_null() {
        Ok(())
    } else {
        Err(format!("expected {key} to be null, got {found}"))
    }
}

#[then(regex = r#"^JSON row (\d+) has an empty "([^"]+)"$"#)]
fn json_row_empty(world: &mut BehaviourWorld, index: usize, key: String) -> Result<(), String> {
    let found = field(&row(world, index)?, &key)?;
    if found.as_array().is_some_and(|a| a.is_empty()) {
        Ok(())
    } else {
        Err(format!("expected {key} to be an empty array, got {found}"))
    }
}

#[then(regex = r#"^JSON row (\d+) has "([^"]+)" set to (-?\d+)$"#)]
fn json_row_number(
    world: &mut BehaviourWorld,
    index: usize,
    key: String,
    expected: i64,
) -> Result<(), String> {
    let found = field(&row(world, index)?, &key)?;
    if found == serde_json::json!(expected) {
        Ok(())
    } else {
        Err(format!(
            "expected {key} to be the raw number {expected}, got {found}"
        ))
    }
}
