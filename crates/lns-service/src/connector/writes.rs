//! The paths installed connectors would write, carried into the guest so the boot can make room for them (§3.1.11).

use lns_artifact::connector::ConnectorDefinition;

use crate::runtime_layer::{RuntimeFileSpec, RuntimeSource};

/// One path an installed connector would write, and the connector to name when a bind leaves no room for it (§3.1.11).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WrittenPath {
    pub connector: String,
    pub path: String,
}

/// Every `~/`-anchored path a method of an installed connector would write, in the spelling the grant itself uses; a method this version cannot offer is never granted, so it claims nothing.
pub fn paths_installed_connectors_write(definitions: &[ConnectorDefinition]) -> Vec<WrittenPath> {
    let mut written: Vec<WrittenPath> = definitions
        .iter()
        .flat_map(|definition| {
            definition
                .spec
                .methods
                .iter()
                .filter(|method| method.is_offerable())
                .flat_map(|method| &method.filesets)
                .flat_map(|fileset| {
                    fileset
                        .inline
                        .iter()
                        .flatten()
                        .map(|(name, _)| WrittenPath {
                            connector: definition.name.clone(),
                            path: lns_artifact::connector::guest_file(&fileset.guest_path, name),
                        })
                })
        })
        .collect();
    written.sort();
    written.dedup();
    written
}

/// No installed connector writes a file, so the guest has nothing to make room for and gets no manifest.
pub fn written_paths_manifest(written: &[WrittenPath]) -> Option<RuntimeFileSpec> {
    if written.is_empty() {
        return None;
    }
    let body: String = written
        .iter()
        .map(|entry| format!("{}\t{}\n", entry.connector, entry.path))
        .collect();
    Some(RuntimeFileSpec {
        guest_path: lns_placement::CONNECTOR_WRITES_MANIFEST.into(),
        mode: 0o444,
        source: RuntimeSource::Bytes(body.into_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(connector: &str, path: &str) -> WrittenPath {
        WrittenPath {
            connector: connector.into(),
            path: path.into(),
        }
    }

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
    fn a_written_path_is_what_the_grant_writes_so_the_boot_and_the_grant_cannot_drift() {
        let paths = paths_installed_connectors_write(&[definition(
            "claude",
            "~/.claude",
            &[".credentials.json"],
        )]);
        assert_eq!(
            paths,
            [written("claude", "~/.claude/.credentials.json")],
            "the refusal has to name the connector, so the path travels with it"
        );
    }

    #[test]
    fn every_installed_connector_and_every_file_of_a_method_is_counted() {
        let paths = paths_installed_connectors_write(&[
            definition(
                "claude",
                "~/.claude",
                &[".credentials.json", "settings.json"],
            ),
            definition("other", "~/.other", &["auth.json"]),
        ]);
        assert_eq!(
            paths,
            [
                written("claude", "~/.claude/.credentials.json"),
                written("claude", "~/.claude/settings.json"),
                written("other", "~/.other/auth.json"),
            ],
            "the boot makes room for every method, because any of them may be the one granted"
        );
    }

    #[test]
    fn two_connectors_writing_one_path_are_two_entries() {
        let paths = paths_installed_connectors_write(&[
            definition("a", "~/.agent", &["auth.json"]),
            definition("b", "~/.agent", &["auth.json"]),
        ]);
        assert_eq!(
            paths,
            [
                written("a", "~/.agent/auth.json"),
                written("b", "~/.agent/auth.json")
            ],
            "one path two connectors write is two claims, because either may be the one refused"
        );
    }

    #[test]
    fn a_method_this_version_cannot_offer_writes_nothing_because_it_can_never_be_granted() {
        let doc = r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"future","spec":{"serves":["api.example"],"methods":[{"name":"later","auth":{"kind":"some_future_kind"},"filesets":[{"inline":{"settings.json":"{}"},"guestPath":"~/.agent"}]}]}}"#;
        let definition =
            lns_artifact::connector::parse(doc.as_bytes()).expect("an unknown kind still parses");
        assert!(
            paths_installed_connectors_write(&[definition]).is_empty(),
            "splitting a bind for a grant this version cannot make is cost with no cover"
        );
    }

    #[test]
    fn the_manifest_carries_one_claim_per_line_for_the_guest_to_resolve() {
        let spec = written_paths_manifest(&[
            written("claude", "~/.claude/.credentials.json"),
            written("other", "~/.a/b"),
        ])
        .expect("claims make a manifest");
        assert_eq!(spec.guest_path, lns_placement::CONNECTOR_WRITES_MANIFEST);
        assert_eq!(
            spec.source
                .as_bytes()
                .map(String::from_utf8_lossy)
                .as_deref(),
            Some("claude\t~/.claude/.credentials.json\nother\t~/.a/b\n"),
            "one line per claim, each naming the connector"
        );
    }

    #[test]
    fn nothing_written_ships_no_manifest_so_an_ordinary_run_carries_nothing_extra() {
        assert!(written_paths_manifest(&[]).is_none());
    }
}
