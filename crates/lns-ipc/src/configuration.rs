use serde::{Deserialize, Serialize};

use crate::SourceContribution;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationSources {
    pub definition: String,
    pub mixins: Vec<String>,
    pub added_mixins: Vec<String>,
    pub contributions: Vec<SourceContribution>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxConfiguration {
    pub sources: Option<ConfigurationSources>,
    pub document: String,
    pub decisions: String,
    pub grants: Vec<ConfigurationGrant>,
    pub rules: Vec<ConfigurationRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationGrant {
    pub name: String,
    pub variables: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationRule {
    pub table: String,
    pub source: String,
    pub rule: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_native_configuration_fixture_is_the_rust_wire_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/macos/Tests/LNSClientTests/Fixtures/configuration.json"
        ))
        .unwrap();
        let requests: Vec<crate::Request> =
            serde_json::from_value(fixture["requests"].clone()).unwrap();
        let responses: Vec<crate::Response> =
            serde_json::from_value(fixture["responses"].clone()).unwrap();
        assert_eq!(serde_json::to_value(requests).unwrap(), fixture["requests"]);
        assert_eq!(
            serde_json::to_value(responses).unwrap(),
            fixture["responses"]
        );
    }
}
