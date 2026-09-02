use cucumber::{then, when};

use crate::world::BehaviourWorld;

fn cmdline_has(w: &mut BehaviourWorld, token: &str) -> Result<(), String> {
    let cmdline = w.bind().cmdline();
    if cmdline.split_whitespace().any(|t| t == token) {
        Ok(())
    } else {
        Err(format!("expected token {token:?} in cmdline: {cmdline}"))
    }
}

#[when(expr = "a run requests host bind {string} at {string}")]
async fn request(w: &mut BehaviourWorld, source: String, target: String) {
    w.bind().request(&source, &target, false, &[]);
}

#[when(expr = "a run requests host bind {string} at {string} read-only")]
async fn request_ro(w: &mut BehaviourWorld, source: String, target: String) {
    w.bind().request(&source, &target, true, &[]);
}

#[when(expr = "a run requests host bind {string} at {string} and {string} at {string}")]
async fn request_two(
    w: &mut BehaviourWorld,
    a_src: String,
    a_tgt: String,
    b_src: String,
    b_tgt: String,
) {
    w.bind().request(&a_src, &a_tgt, false, &[]);
    w.bind().request(&b_src, &b_tgt, false, &[]);
}

#[when(expr = "a run requests host bind {string} at {string} dropping {string}")]
async fn request_dropping(w: &mut BehaviourWorld, source: String, target: String, drop: String) {
    w.bind().request(&source, &target, false, &[&drop]);
}

#[when(expr = "a run requests host bind {string} at {string} seeding {string}")]
async fn request_seeding(w: &mut BehaviourWorld, source: String, target: String, seed: String) {
    w.bind().request_seeding(&source, &target, &[], &[&seed]);
}

#[when(expr = "a host bind {string} at {string} is recorded in the audit chain")]
async fn record_bind_audit(w: &mut BehaviourWorld, source: String, target: String) {
    w.bind().record_audit(&source, &target, &[], &[]);
}

#[when(
    expr = "a host bind {string} at {string} exposing {string} and dropping {string} is recorded in the audit chain"
)]
async fn record_bind_audit_with_secrets(
    w: &mut BehaviourWorld,
    source: String,
    target: String,
    exposed: String,
    dropped: String,
) {
    w.bind()
        .record_audit(&source, &target, &[&exposed], &[&dropped]);
}

#[then(expr = "the spec carries a virtio-fs share for {string} at {string}")]
async fn share_at_target(
    w: &mut BehaviourWorld,
    _source: String,
    target: String,
) -> Result<(), String> {
    cmdline_has(w, "bind.0.tag=lns-bind-0")?;
    cmdline_has(w, &format!("bind.0.target={target}"))
}

#[then(expr = "that share is writable")]
async fn share_writable(w: &mut BehaviourWorld) -> Result<(), String> {
    cmdline_has(w, "bind.0.ro=0")
}

#[then(expr = "the spec marks the share for {string} read-only")]
async fn share_read_only(w: &mut BehaviourWorld, _source: String) -> Result<(), String> {
    cmdline_has(w, "bind.0.ro=1")
}

#[then(expr = "the spec carries two virtio-fs shares with distinct tags")]
async fn two_distinct_tags(w: &mut BehaviourWorld) -> Result<(), String> {
    cmdline_has(w, "bind.0.tag=lns-bind-0")?;
    cmdline_has(w, "bind.1.tag=lns-bind-1")
}

#[then(expr = "the content share tag is left untouched")]
async fn content_tag_untouched(w: &mut BehaviourWorld) -> Result<(), String> {
    cmdline_has(w, "content.tag=lns-content")
}

#[then(expr = "the bind spec for {string} lists {string} in its seeded paths")]
async fn bind_lists_seed(
    w: &mut BehaviourWorld,
    target: String,
    name: String,
) -> Result<(), String> {
    cmdline_has(w, &format!("bind.0.target={target}"))?;
    cmdline_has(w, "bind.0.seeds=1")?;
    cmdline_has(w, &format!("bind.0.seed.0={name}"))
}

#[then(expr = "the bind spec for {string} declares no seeded paths")]
async fn bind_lists_no_seed(w: &mut BehaviourWorld, target: String) -> Result<(), String> {
    cmdline_has(w, &format!("bind.0.target={target}"))?;
    cmdline_has(w, "bind.0.seeds=0")
}

#[then(expr = "the bind spec for {string} lists {string} in its dropped paths")]
async fn bind_lists_drop(
    w: &mut BehaviourWorld,
    target: String,
    name: String,
) -> Result<(), String> {
    cmdline_has(w, &format!("bind.0.target={target}"))?;
    cmdline_has(w, &format!("bind.0.drop.0={name}"))
}

#[then(expr = "the audit chain records the host source {string} and target {string}")]
async fn audit_records(
    w: &mut BehaviourWorld,
    source: String,
    target: String,
) -> Result<(), String> {
    let content = w.bind().audit_contents();
    if content.contains(&format!("\"lns_source\":\"{source}\""))
        && content.contains(&format!("\"lns_target\":\"{target}\""))
    {
        Ok(())
    } else {
        Err(format!("audit chain missing source/target: {content}"))
    }
}

#[then(expr = "the audit chain records {string} as exposed and {string} as dropped")]
async fn audit_records_disposition(
    w: &mut BehaviourWorld,
    exposed: String,
    dropped: String,
) -> Result<(), String> {
    let content = w.bind().audit_contents();
    if content.contains(&format!("\"lns_exposed_secrets\":[\"{exposed}\"]"))
        && content.contains(&format!("\"lns_dropped_secrets\":[\"{dropped}\"]"))
    {
        Ok(())
    } else {
        Err(format!("audit chain missing secret disposition: {content}"))
    }
}
