use lns_policy::{Policy, Verdict};
use serde_json::{Value, json};

use crate::Error;

pub fn policy_data(policy: &Policy) -> Result<String, Error> {
    policy.network.validate_local_transport()?;
    policy.network.validate_binary_scopes()?;
    let mut rules = Vec::new();
    for rule in &policy.network.egress.tcp {
        rule.validate()?;
        rules.push(compile_rule(
            &rule.match_pattern,
            rule.verdict,
            &rule.binaries,
        )?);
    }
    for rule in &policy.network.egress.http {
        if rule.scheme.is_some() || rule.tls_terminate || !rule.rules.is_empty() {
            return Err("spike supports destination admission only; HTTP inspection needs a separate adapter".into());
        }
        rules.push(compile_rule(
            &rule.match_pattern,
            rule.verdict,
            &rule.binaries,
        )?);
    }
    Ok(json!({"lns_rules": rules, "network_policies": {}}).to_string())
}

fn compile_rule(
    pattern: &str,
    verdict: Verdict,
    binaries: &Option<Vec<String>>,
) -> Result<Value, Error> {
    if pattern.contains('/') {
        return Err("CIDR matching is outside this spike".into());
    }
    let (host, port) = lns_policy::matching::split_destination(pattern);
    let port = port.map(str::parse::<u16>).transpose()?;
    let host = host.to_ascii_lowercase();
    let matcher = if host == "*" {
        json!({"kind":"any"})
    } else if let Some(suffix) = host.strip_prefix("*.") {
        json!({"kind":"suffix", "suffix":suffix})
    } else if host.contains("*.") {
        let (prefix, suffix) = host.split_once('*').ok_or("invalid wildcard")?;
        json!({"kind":"middle", "prefix":prefix, "suffix":suffix})
    } else {
        json!({"kind":"exact", "host":host})
    };
    let binaries = binaries.as_ref().map(|paths| {
        paths
            .iter()
            .map(|path| {
                std::path::Path::new(path)
                    .components()
                    .collect::<std::path::PathBuf>()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>()
    });
    Ok(json!({"matcher":matcher, "port":port, "verdict":verdict, "binaries":binaries}))
}
