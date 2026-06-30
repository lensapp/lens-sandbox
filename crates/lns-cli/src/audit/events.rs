use lns_ipc::{ApprovalKind, Decision, LedgerEvent};

use super::auth_word;

pub(super) fn detail(event: &LedgerEvent) -> String {
    match event {
        LedgerEvent::Approval {
            kind,
            target,
            decision,
            reason,
            ..
        } => {
            let base = format!(
                "{} {} {target}",
                approval_kind_word(*kind),
                decision_word(*decision)
            );
            match reason {
                Some(reason) => format!("{base}  [{reason}]"),
                None => base,
            }
        }
        LedgerEvent::Connection {
            integration,
            auth,
            account,
            scopes,
            ..
        } => {
            let who = account.as_deref().unwrap_or("-");
            format!(
                "connect {integration} ({}) {who} [{}]",
                auth_word(*auth),
                scopes.join(", ")
            )
        }
        LedgerEvent::CredentialUse {
            integration,
            fp,
            dest,
            ..
        } => {
            let key = fp
                .as_deref()
                .map(|f| format!(" fp {f}"))
                .unwrap_or_default();
            format!("use {integration}{key} → {}", dest.join(", "))
        }
    }
}

fn decision_word(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "allow-once",
        Decision::AllowAlways => "allow-always",
        Decision::DenyOnce => "deny-once",
        Decision::DenyAlways => "deny-always",
        Decision::Allow => "allow",
        Decision::Deny => "deny",
    }
}

fn approval_kind_word(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::Network => "network",
        ApprovalKind::Credential => "credential",
        ApprovalKind::Integration => "integration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::AuthKind;

    #[test]
    fn an_approval_with_a_reason_brackets_it() {
        let event = LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: "api.foo.com:443".into(),
            decision: Decision::AllowAlways,
            reason: Some("policy-ambiguous".into()),
            integration: None,
        };
        assert_eq!(
            detail(&event),
            "network allow-always api.foo.com:443  [policy-ambiguous]"
        );
    }

    #[test]
    fn a_connection_renders_auth_account_and_scopes() {
        let event = LedgerEvent::Connection {
            integration: "some-oauth".into(),
            auth: AuthKind::Oauth,
            account: Some("@hchen".into()),
            scopes: vec!["repo".into(), "read:org".into()],
            expires: Some("2026-07-29T00:00:00Z".into()),
        };
        assert_eq!(
            detail(&event),
            "connect some-oauth (oauth) @hchen [repo, read:org]"
        );
    }

    #[test]
    fn a_credential_use_renders_the_fingerprint_and_dests() {
        let event = LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: Some("9c2f1a3d".into()),
            dest: vec!["api.some-provider.example".into()],
        };
        assert_eq!(
            detail(&event),
            "use some-provider fp 9c2f1a3d → api.some-provider.example"
        );
    }

    #[test]
    fn an_approval_without_a_reason_omits_the_bracket() {
        let event = LedgerEvent::Approval {
            kind: ApprovalKind::Credential,
            target: "some-provider".into(),
            decision: Decision::Deny,
            reason: None,
            integration: Some("some-provider".into()),
        };
        assert_eq!(detail(&event), "credential deny some-provider");
    }

    #[test]
    fn every_decision_and_auth_word_is_mapped() {
        for d in [
            Decision::AllowOnce,
            Decision::AllowAlways,
            Decision::DenyOnce,
            Decision::DenyAlways,
            Decision::Allow,
            Decision::Deny,
        ] {
            assert!(!decision_word(d).is_empty());
        }
        assert_eq!(auth_word(AuthKind::Oauth), "oauth");
        assert_eq!(auth_word(AuthKind::Apikey), "apikey");
        assert_eq!(approval_kind_word(ApprovalKind::Integration), "integration");
    }

    #[test]
    fn a_credential_use_without_a_fingerprint_still_renders() {
        let event = LedgerEvent::CredentialUse {
            integration: "some-provider".into(),
            auth: AuthKind::Apikey,
            fp: None,
            dest: vec![],
        };
        let rendered = detail(&event);
        assert!(
            rendered.starts_with("use some-provider →"),
            "got: {rendered}"
        );
        assert!(
            !rendered.contains("  "),
            "a missing fingerprint must not leave a double space: {rendered}"
        );
    }
}
