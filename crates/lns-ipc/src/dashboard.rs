use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardSandbox {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardEvent {
    pub id: String,
    pub ts: String,
    pub when: String,
    pub run: String,
    pub kind: String,
    pub detail: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardApproval {
    pub entry: crate::ApprovalInfo,
    pub raw: bool,
    pub answers: Vec<crate::ApprovalAnswer>,
    pub grantable: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_swift_dashboard_fixture_is_the_rust_wire_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/macos/Tests/LNSClientTests/Fixtures/dashboard.json"
        ))
        .unwrap();
        let messages: Vec<crate::Response> = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(messages.first(), Some(&crate::Response::DashboardBegin));
        assert_eq!(messages.last(), Some(&crate::Response::DashboardEnd));
        assert_eq!(serde_json::to_value(messages).unwrap(), fixture);
    }
}
