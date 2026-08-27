//! Resolving `lns connector install <REF|PATH>` to the bytes a grant binds to.
//!
//! The service resolves both forms because only it can reach a registry, and a
//! connector's `path` filesets arrive as layers of its own artifact.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// What one connector arrives as: the digest a grant binds to, and the document verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedConnector {
    pub digest: String,
    pub document: Vec<u8>,
}

/// Which of the two forms `<REF|PATH>` named. A path is absolute by the time it reaches the service, because the service's working directory is not the user's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Reference(String),
    Local(PathBuf),
}

impl Source {
    pub fn of(operand: &str) -> Result<Self> {
        if !lns_artifact::sandbox::names_a_local_path(operand) {
            return Ok(Self::Reference(operand.to_string()));
        }
        let path = Path::new(operand);
        if !path.is_absolute() {
            bail!(
                "connector {operand} reached the service as a relative path; the service's working directory is not yours, so `lns` sends the absolute one"
            );
        }
        Ok(Self::Local(lns_artifact::sandbox::fold_path(path)))
    }
}

/// Where a connector's bytes come from: a registry for a reference, this machine for a directory.
pub trait ConnectorSource: Send + Sync {
    fn fetch(
        &self,
        source: &Source,
    ) -> impl std::future::Future<Output = Result<FetchedConnector>> + Send;
}

const LNS_YAML: &str = "lns.yaml";
const README: &str = "README.md";

/// Read a connector directory the way `lns push` would, so a local install carries the digest publishing it would produce and a grant survives that publish (§7.1).
pub fn read_local<F: lns_artifact::walk::SnapshotFs + ?Sized>(
    fs: &F,
    read_document: impl FnOnce(&Path) -> Result<Vec<u8>>,
    dir: &Path,
) -> Result<FetchedConnector> {
    let document = read_document(&dir.join(LNS_YAML))?;
    let connector = lns_artifact::connector::parse(&document)?;
    let mut filesets = Vec::new();
    for (method, path) in lns_artifact::connector::path_filesets(&connector.spec) {
        let entries =
            lns_artifact::walk::walk(fs, &dir.join(path), lns_artifact::spec::Kind::Connector)
                .with_context(|| format!("method {method} fileset {path}"))?;
        filesets.push(entries);
    }
    let readme = readme_beside(fs, dir)?;
    let built = lns_artifact::build::build_artifact(&document, &filesets, readme.as_deref())
        .context("building the connector artifact")?;
    Ok(FetchedConnector {
        digest: built.manifest_digest,
        document,
    })
}

/// A push publishes the `README.md` beside a document as a layer of the same artifact, so the digest depends on it. Absent is `None`; anything else is the caller's problem to see.
fn readme_beside<F: lns_artifact::walk::SnapshotFs + ?Sized>(
    fs: &F,
    dir: &Path,
) -> Result<Option<Vec<u8>>> {
    let path = dir.join(README);
    match fs.read_limited(&path, lns_artifact::build::MAX_README_BYTES) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e)).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_artifact::walk::{DirEntry, SnapshotFs};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MapFs {
        files: BTreeMap<PathBuf, Vec<u8>>,
        deny_reads: bool,
    }

    impl MapFs {
        fn with(entries: &[(&str, &[u8])]) -> Self {
            Self {
                files: entries
                    .iter()
                    .map(|(p, d)| (PathBuf::from(p), d.to_vec()))
                    .collect(),
                ..Self::default()
            }
        }
    }

    impl SnapshotFs for MapFs {
        fn read_limited(&self, path: &Path, _max: u64) -> std::io::Result<Vec<u8>> {
            if self.deny_reads {
                return Err(std::io::Error::other("permission denied"));
            }
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
        }

        fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
            lns_artifact::walk::map_dir_entries(self.files.keys(), dir)
        }
    }

    fn document(filesets: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{{"serves":["api.some-provider.example"],"methods":[{{"name":"token","auth":{{"kind":"token"}},"credentials":[{{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}}]{filesets}}}]}}}}"#
        )
        .into_bytes()
    }

    fn reading(doc: Vec<u8>) -> impl FnOnce(&Path) -> Result<Vec<u8>> {
        move |_| Ok(doc)
    }

    #[test]
    fn a_reference_and_an_absolute_path_are_told_apart() {
        assert_eq!(
            Source::of("ghcr.io/acme/some-provider:1").unwrap(),
            Source::Reference("ghcr.io/acme/some-provider:1".to_string())
        );
        assert_eq!(
            Source::of("/work/some-provider").unwrap(),
            Source::Local(PathBuf::from("/work/some-provider"))
        );
    }

    #[test]
    fn a_relative_path_is_refused_because_the_services_working_directory_is_not_yours() {
        for relative in ["./some-provider", "../some-provider"] {
            let err = Source::of(relative).unwrap_err().to_string();
            assert!(err.contains("relative"), "{relative}: {err}");
        }
    }

    #[test]
    fn a_path_is_folded_so_two_spellings_of_one_directory_are_one_source() {
        assert_eq!(
            Source::of("/work/./mixins/../some-provider").unwrap(),
            Source::Local(PathBuf::from("/work/some-provider"))
        );
    }

    #[test]
    fn a_local_connector_carries_the_digest_publishing_it_would_produce() {
        // A grant binds to the digest, so `install ./dir` then `install <ref>` of those same bytes must resume the grant rather than ask again (§7.1).
        let doc = document("");
        let fetched = read_local(&MapFs::default(), reading(doc.clone()), Path::new("/work"))
            .expect("a connector with no path fileset reads");
        let published = lns_artifact::build::build_artifact(&doc, &[], None).expect("build");
        assert_eq!(fetched.digest, published.manifest_digest);
        assert_eq!(fetched.document, doc);
    }

    #[test]
    fn a_readme_beside_the_document_reaches_the_digest_the_way_a_push_publishes_it() {
        // A push publishes the README as a layer, so omitting it here would make `install ./dir`, publish, `install <ref>` ask again — the grant continuity §7.1 promises.
        let doc = document("");
        let readme = b"# some-provider\n";
        let fs = MapFs::with(&[("/work/README.md", readme)]);
        let fetched = read_local(&fs, reading(doc.clone()), Path::new("/work")).expect("reads");
        let published = lns_artifact::build::build_artifact(&doc, &[], Some(readme))
            .expect("what a push of this directory publishes");
        assert_eq!(fetched.digest, published.manifest_digest);
    }

    #[test]
    fn a_directory_with_no_readme_publishes_the_digest_that_has_none() {
        let doc = document("");
        let fetched =
            read_local(&MapFs::default(), reading(doc.clone()), Path::new("/work")).expect("reads");
        let published = lns_artifact::build::build_artifact(&doc, &[], None).expect("build");
        assert_eq!(fetched.digest, published.manifest_digest);
    }

    #[test]
    fn a_readme_that_cannot_be_read_for_another_reason_surfaces_rather_than_changing_the_digest() {
        // Treating every error as "absent" would silently publish a different digest than the push does.
        let denied = MapFs {
            deny_reads: true,
            ..MapFs::default()
        };
        let err = read_local(&denied, reading(document("")), Path::new("/work"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("README.md"), "{err}");
    }

    #[test]
    fn a_path_fileset_is_snapshotted_into_the_digest() {
        // The digest must cover the files, or two connectors differing only in a fileset would share one grant.
        let doc = document(
            r#","filesets":[{"path":"./some-provider","guestPath":"/home/agent/.some-provider"}]"#,
        );
        let with_file = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            br#"{"token":"some_LNSPLACEHOLDER0000000000"}"#,
        )]);
        let other_file = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            br#"{"token":"some_LNSPLACEHOLDER0000000000","extra":1}"#,
        )]);

        let one = read_local(&with_file, reading(doc.clone()), Path::new("/work")).expect("one");
        let two = read_local(&other_file, reading(doc.clone()), Path::new("/work")).expect("two");

        assert_ne!(
            one.digest, two.digest,
            "a fileset's content has to reach the digest a grant binds to"
        );
    }

    #[test]
    fn a_missing_path_fileset_names_the_method_and_the_path() {
        let doc = document(r#","filesets":[{"path":"./absent","guestPath":"/home/agent/.x"}]"#);
        let err = read_local(&MapFs::default(), reading(doc), Path::new("/work"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("token"), "{err}");
        assert!(err.contains("./absent"), "{err}");
    }

    #[test]
    fn a_document_that_is_not_a_connector_is_refused_before_anything_is_built() {
        let sandbox =
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1"}}"#;
        assert!(
            read_local(
                &MapFs::default(),
                reading(sandbox.to_vec()),
                Path::new("/work")
            )
            .is_err()
        );
    }

    #[test]
    fn a_document_that_cannot_be_read_surfaces_that_rather_than_building() {
        let err = read_local(
            &MapFs::default(),
            |path| anyhow::bail!("reading {}", path.display()),
            Path::new("/work"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("/work/lns.yaml"), "{err}");
    }
}
