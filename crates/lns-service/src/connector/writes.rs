//! The paths installed connectors would write, carried into the guest so the boot can make room for them (§3.1.11).

use lns_artifact::connector::ConnectorDefinition;

use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

/// Where the guest reads the claims: only it resolves `~`, so only it can decide which bind entries to leave unmounted.
pub const CLAIMS_MANIFEST_PATH: &str = "/.lens/connector-claims";

/// Every `~/`-anchored path a method of an installed connector would write, in the spelling the grant itself uses; a method this version cannot offer is never granted, so it claims nothing.
pub fn installed_claims(definitions: &[ConnectorDefinition]) -> Vec<String> {
    let mut claims: Vec<String> = definitions
        .iter()
        .flat_map(|definition| &definition.spec.methods)
        .filter(|method| method.is_offerable())
        .flat_map(|method| &method.filesets)
        .flat_map(|fileset| {
            fileset
                .inline
                .iter()
                .flatten()
                .map(|(name, _)| lns_artifact::connector::guest_file(&fileset.guest_path, name))
        })
        .collect();
    claims.sort();
    claims.dedup();
    claims
}

/// No installed connector writes a file, so the guest has nothing to make room for and gets no manifest.
pub fn claims_manifest(claims: &[String]) -> Option<RuntimeFileSpec> {
    if claims.is_empty() {
        return None;
    }
    let body: String = claims.iter().map(|claim| format!("{claim}\n")).collect();
    Some(RuntimeFileSpec {
        guest_path: CLAIMS_MANIFEST_PATH.into(),
        mode: 0o444,
        source: RuntimeSource::Bytes(body.into_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACEHOLDER: &str = "some_LNSPLACEHOLDER0000000000";

    /// A secret-shaped name needs a placeholder its own method declares (§3.2.5), so a realistic fixture declares one.
    fn definition(name: &str, guest_path: &str, files: &[&str]) -> ConnectorDefinition {
        let inline: String = files
            .iter()
            .map(|file| format!(r#""{file}":"{{\"token\":\"{PLACEHOLDER}\"}}","#))
            .collect();
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"connector","name":"{name}","spec":{{"serves":["api.example"],"methods":[{{"name":"token","auth":{{"kind":"token"}},"credentials":[{{"envVar":"SOME_TOKEN","placeholder":"{PLACEHOLDER}","injections":[{{"kind":"bearer_header","domain":"api.example"}}]}}],"filesets":[{{"inline":{{{}}},"guestPath":"{guest_path}"}}]}}]}}}}"#,
            inline.trim_end_matches(',')
        );
        lns_artifact::connector::parse(doc.as_bytes()).expect("a connector the tests author")
    }

    #[test]
    fn a_claim_is_the_path_the_grant_writes_so_the_boot_and_the_grant_cannot_drift() {
        let claims = installed_claims(&[definition("claude", "~/.claude", &[".credentials.json"])]);
        assert_eq!(claims, ["~/.claude/.credentials.json"]);
    }

    #[test]
    fn every_installed_connector_and_every_file_of_a_method_is_counted() {
        let claims = installed_claims(&[
            definition(
                "claude",
                "~/.claude",
                &[".credentials.json", "settings.json"],
            ),
            definition("other", "~/.other", &["auth.json"]),
        ]);
        assert_eq!(
            claims,
            [
                "~/.claude/.credentials.json",
                "~/.claude/settings.json",
                "~/.other/auth.json",
            ],
            "the boot makes room for every method, because any of them may be the one granted"
        );
    }

    #[test]
    fn two_connectors_claiming_one_path_name_it_once() {
        let claims = installed_claims(&[
            definition("a", "~/.agent", &["auth.json"]),
            definition("b", "~/.agent", &["auth.json"]),
        ]);
        assert_eq!(claims, ["~/.agent/auth.json"]);
    }

    #[test]
    fn a_method_this_version_cannot_offer_claims_nothing_because_it_can_never_be_granted() {
        let doc = r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"future","spec":{"serves":["api.example"],"methods":[{"name":"later","auth":{"kind":"some_future_kind"},"filesets":[{"inline":{"settings.json":"{}"},"guestPath":"~/.agent"}]}]}}"#;
        let definition =
            lns_artifact::connector::parse(doc.as_bytes()).expect("an unknown kind still parses");
        assert!(
            installed_claims(&[definition]).is_empty(),
            "splitting a bind for a grant this version cannot make is cost with no cover"
        );
    }

    #[test]
    fn the_manifest_carries_one_claim_per_line_for_the_guest_to_resolve() {
        let spec = claims_manifest(&["~/.claude/.credentials.json".into(), "~/.a/b".into()])
            .expect("claims make a manifest");
        assert_eq!(spec.guest_path, CLAIMS_MANIFEST_PATH);
        match spec.source {
            RuntimeSource::Bytes(body) => assert_eq!(
                String::from_utf8(body).expect("utf8"),
                "~/.claude/.credentials.json\n~/.a/b\n"
            ),
            other => panic!("the manifest must be inline bytes, got {other:?}"),
        }
    }

    #[test]
    fn no_claims_ship_no_manifest_so_an_ordinary_run_carries_nothing_extra() {
        assert!(claims_manifest(&[]).is_none());
    }
}
