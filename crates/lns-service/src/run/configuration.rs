use std::collections::BTreeMap;

use anyhow::{Context, Result};
use lns_ipc::{
    ConfigurationGrant, ConfigurationRule, ConfigurationSources, ContributionBlock,
    SandboxConfiguration,
};
use lns_policy::Policy;
use serde_json::{Value, json};

use crate::approval_flow::protocol::GrantedPayload;
use crate::run_record::RunRecord;

pub async fn preview(
    document: &[u8],
    home: &crate::artifact::mixin::Locator,
    mixins: &[String],
    source: &impl crate::artifact::mixin::MixinSource,
) -> Result<SandboxConfiguration> {
    let resolution = crate::artifact::mixin::resolve(document, mixins, home, source, None).await?;
    let sources = ConfigurationSources {
        definition: home.key(),
        mixins: resolution.mixins,
        added_mixins: resolution.pinned_extra,
        contributions: crate::artifact::mixin::on_the_wire(&resolution.contributions),
    };
    project(
        serde_json::from_slice(&resolution.document)?,
        Some(sources),
        &json!({}),
        &BTreeMap::new(),
    )
}

pub fn inspect(
    record: &RunRecord,
    decisions: &Policy,
    grants: &BTreeMap<String, GrantedPayload>,
) -> Result<SandboxConfiguration> {
    let rendered = super::save::render(
        record,
        &Policy::default(),
        lns_ipc::SaveKind::Sandbox,
        &record.name,
    )?;
    let mut document: Value = serde_yaml::from_str(&rendered)?;
    if let Some(authored) = &record.args.authored_egress {
        document["spec"]["egress"] =
            serde_json::from_str(authored).context("reading the recorded network baseline")?;
    }
    let bytes = decisions.document_bytes(std::path::Path::new("decisions"))?;
    let decision_document: Value = serde_yaml::from_slice(&bytes)?;
    let mut config = project(
        document,
        record
            .args
            .configuration_sources
            .as_deref()
            .cloned()
            .map(without_recorded_decisions),
        &decision_document["spec"],
        grants,
    )?;
    config.decisions = serde_json::to_string(&decision_document)?;
    Ok(config)
}

fn without_recorded_decisions(mut sources: ConfigurationSources) -> ConfigurationSources {
    const RUN_DECISIONS_SOURCE: &str = "decisions.yaml";
    sources
        .mixins
        .retain(|source| source != RUN_DECISIONS_SOURCE);
    sources
        .contributions
        .retain(|entry| entry.source != RUN_DECISIONS_SOURCE);
    sources
}

pub fn project(
    document: Value,
    sources: Option<ConfigurationSources>,
    decisions: &Value,
    grants: &BTreeMap<String, GrantedPayload>,
) -> Result<SandboxConfiguration> {
    let mut rules = Vec::new();
    append_rules(&mut rules, &decisions["egress"], "Your decision", None)?;
    let mut disclosed_grants = Vec::new();
    for (name, grant) in grants {
        append_rules(
            &mut rules,
            &serde_json::to_value(&grant.egress.network.egress)?,
            &format!("Connector: {name}"),
            None,
        )?;
        disclosed_grants.push(ConfigurationGrant {
            name: name.clone(),
            variables: grant.env.keys().cloned().collect(),
            files: grant.files.iter().map(|file| file.path.clone()).collect(),
        });
    }
    append_rules(
        &mut rules,
        &document["spec"]["egress"],
        "Definition and mixins",
        sources.as_ref(),
    )?;
    Ok(SandboxConfiguration {
        sources,
        document: serde_json::to_string(&document)?,
        decisions: serde_json::to_string(&json!({"spec": decisions}))?,
        grants: disclosed_grants,
        rules,
    })
}

fn append_rules(
    output: &mut Vec<ConfigurationRule>,
    egress: &Value,
    fallback: &str,
    sources: Option<&ConfigurationSources>,
) -> Result<()> {
    let mut contributions: Vec<_> = sources
        .into_iter()
        .flat_map(|s| &s.contributions)
        .filter(|c| c.block == ContributionBlock::Egress)
        .collect();
    for table in ["http", "tcp"] {
        for rule in egress[table].as_array().into_iter().flatten() {
            let key = format!(
                "{} {}",
                rule["verdict"].as_str().unwrap_or_default(),
                rule["match"].as_str().unwrap_or_default()
            );
            let source = contributions
                .iter()
                .position(|c| c.key == key)
                .map(|index| contributions.remove(index).source.as_str())
                .unwrap_or(fallback);
            output.push(ConfigurationRule {
                table: table.into(),
                source: source.into(),
                rule: serde_json::to_string(rule)?,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restarted_runs_old_decision_is_not_misreported_as_an_authored_rule() {
        let mut record = crate::run_record::test_record("aa01");
        record.resolved_document = Some(json!({
            "apiVersion": "lns.run/v1", "kind": "sandbox", "name": "reviewer",
            "spec": {"image": "alpine:3.20", "egress": {"http": [{"match": "old.example.com", "verdict": "deny"}]}}
        }).to_string());
        record.args.authored_egress = Some(
            json!({"http": [{"match": "docs.example.com", "verdict": "allow"}], "tcp": []})
                .to_string(),
        );
        record.args.configuration_sources = Some(Box::new(ConfigurationSources {
            definition: "sandbox@sha256:one".into(),
            mixins: vec!["decisions.yaml".into()],
            added_mixins: vec![],
            contributions: ["decisions.yaml", "the sandbox"]
                .into_iter()
                .map(|source| lns_ipc::SourceContribution {
                    block: ContributionBlock::Egress,
                    key: "allow docs.example.com".into(),
                    source: source.into(),
                    note: None,
                    displaced: vec![],
                })
                .collect(),
        }));
        let config = inspect(&record, &Policy::default(), &BTreeMap::new()).unwrap();
        assert_eq!(config.rules.len(), 1);
        assert!(config.rules[0].rule.contains("docs.example.com"));
        assert_eq!(config.rules[0].source, "the sandbox");
        assert!(config.sources.as_ref().unwrap().mixins.is_empty());
        assert!(!config.document.contains("old.example.com"));
        record.args.authored_egress = Some("damaged".into());
        assert!(
            inspect(&record, &Policy::default(), &BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("recorded network baseline")
        );
    }
}
