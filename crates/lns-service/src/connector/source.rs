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

/// Which of the two forms `<REF|PATH>` named. `Local` is the connector's document, absolute by the time it reaches the service, because the service's working directory is not the user's.
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
        Ok(Self::Local(document_at(&lns_artifact::sandbox::fold_path(
            path,
        ))))
    }
}

/// The document a path names: itself when it is one, else the `lns.yaml` in the directory it names (cli-spec §2.4). Syntactic, because the service decides this and only the user's filesystem could answer it otherwise.
fn document_at(path: &Path) -> PathBuf {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("yaml" | "yml") => path.to_path_buf(),
        _ => path.join(LNS_YAML),
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

/// Read a connector the way `lns push` would, so a local install carries the digest publishing it would produce and a grant survives that publish (§7.1).
///
/// Takes the document, and roots the filesets and the README at the directory holding it — a push does the same for a document under any name, so the digest matches either way.
pub fn read_local<F: lns_artifact::walk::SnapshotFs + ?Sized>(
    fs: &F,
    read_document: impl FnOnce(&Path) -> Result<Vec<u8>>,
    document_path: &Path,
) -> Result<FetchedConnector> {
    let dir = document_path.parent().unwrap_or(Path::new(""));
    let document = read_document(document_path)?;
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
            Source::Local(PathBuf::from("/work/some-provider/lns.yaml"))
        );
    }

    #[test]
    fn the_three_spellings_a_user_may_type_each_resolve_to_what_they_named() {
        // A document, a directory, and a registry reference. A bare `path/to/file.yaml` is deliberately not a fourth: it is a reference, because a repository name may contain dots and slashes.
        assert_eq!(
            Source::of("/work/docs/lns.yaml").unwrap(),
            Source::Local(PathBuf::from("/work/docs/lns.yaml")),
            "a document is taken as itself"
        );
        assert_eq!(
            Source::of("/work/docs").unwrap(),
            Source::Local(PathBuf::from("/work/docs/lns.yaml")),
            "a directory is taken as the document inside it"
        );
        assert_eq!(
            Source::of("acme/docs:1").unwrap(),
            Source::Reference("acme/docs:1".to_string()),
            "anything the user did not spell as a path is a reference"
        );
        assert_eq!(
            Source::of("/work/docs/lns.dev.yaml").unwrap(),
            Source::Local(PathBuf::from("/work/docs/lns.dev.yaml")),
            "the document's name is not what makes it one"
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
            Source::Local(PathBuf::from("/work/some-provider/lns.yaml"))
        );
    }

    #[test]
    fn a_local_connector_carries_the_digest_publishing_it_would_produce() {
        // A grant binds to the digest, so `install ./dir` then `install <ref>` of those same bytes must resume the grant rather than ask again (§7.1).
        let doc = document("");
        let fetched = read_local(
            &MapFs::default(),
            reading(doc.clone()),
            Path::new("/work/lns.yaml"),
        )
        .expect("a connector with no path fileset reads");
        let published = lns_artifact::build::build_artifact(&doc, &[], None).expect("build");
        assert_eq!(fetched.digest, published.manifest_digest);
        assert_eq!(fetched.document, doc);
    }

    /// Records which path the caller asked to read, so a test can pin *which* document was taken.
    fn reading_from(
        doc: Vec<u8>,
        seen: std::rc::Rc<std::cell::RefCell<Option<PathBuf>>>,
    ) -> impl FnOnce(&Path) -> Result<Vec<u8>> {
        move |path| {
            *seen.borrow_mut() = Some(path.to_path_buf());
            Ok(doc)
        }
    }

    #[test]
    fn a_path_naming_the_document_itself_reads_that_document() {
        // cli-spec §2.4 makes a PATH "a path to a local document", so naming the file is one of the spellings a user may type.
        let doc = document("");
        let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
        let fetched = read_local(
            &MapFs::default(),
            reading_from(doc.clone(), seen.clone()),
            Path::new("/work/lns.yaml"),
        )
        .expect("a document path reads");

        assert_eq!(seen.borrow().as_deref(), Some(Path::new("/work/lns.yaml")));
        assert_eq!(fetched.document, doc);
    }

    #[test]
    fn a_document_named_something_else_is_still_a_document() {
        // The spec's own example is `lns.dev.yaml`, so the name is not the thing that makes it one.
        let doc = document("");
        let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
        read_local(
            &MapFs::default(),
            reading_from(doc, seen.clone()),
            Path::new("/work/lns.dev.yaml"),
        )
        .expect("reads");

        assert_eq!(
            seen.borrow().as_deref(),
            Some(Path::new("/work/lns.dev.yaml"))
        );
    }

    #[test]
    fn a_document_under_any_name_digests_the_directory_it_sits_in() {
        // §7.1: the digest must be what a push of that directory publishes, and `lns push -f ./x/lns.dev.yaml` roots both the filesets and the README at `./x`. Rooting anywhere else here would make the grant stop surviving a publish.
        let doc = document(r#","filesets":[{"path":"./seed","guestPath":"~/.some-provider"}]"#);
        let readme = b"# some-provider\n";
        let fs = MapFs::with(&[
            ("/work/README.md", readme),
            ("/work/seed/config", b"token: some_LNSPLACEHOLDER0000000000"),
        ]);

        let fetched = read_local(&fs, reading(doc.clone()), Path::new("/work/lns.dev.yaml"))
            .expect("a document under another name reads");

        let seeded = lns_artifact::walk::walk(
            &fs,
            Path::new("/work/seed"),
            lns_artifact::spec::Kind::Connector,
        )
        .expect("the directory a push would pack");
        let published = lns_artifact::build::build_artifact(&doc, &[seeded], Some(readme))
            .expect("what a push of that directory publishes");
        assert_eq!(fetched.digest, published.manifest_digest);
    }

    #[test]
    fn a_readme_beside_the_document_reaches_the_digest_the_way_a_push_publishes_it() {
        // A push publishes the README as a layer, so omitting it here would make `install ./dir`, publish, `install <ref>` ask again — the grant continuity §7.1 promises.
        let doc = document("");
        let readme = b"# some-provider\n";
        let fs = MapFs::with(&[("/work/README.md", readme)]);
        let fetched =
            read_local(&fs, reading(doc.clone()), Path::new("/work/lns.yaml")).expect("reads");
        let published = lns_artifact::build::build_artifact(&doc, &[], Some(readme))
            .expect("what a push of this directory publishes");
        assert_eq!(fetched.digest, published.manifest_digest);
    }

    #[test]
    fn a_directory_with_no_readme_publishes_the_digest_that_has_none() {
        let doc = document("");
        let fetched = read_local(
            &MapFs::default(),
            reading(doc.clone()),
            Path::new("/work/lns.yaml"),
        )
        .expect("reads");
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
        let err = read_local(&denied, reading(document("")), Path::new("/work/lns.yaml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("README.md"), "{err}");
    }

    #[test]
    fn a_path_fileset_is_snapshotted_into_the_digest() {
        // The digest must cover the files, or two connectors differing only in a fileset would share one grant.
        let doc =
            document(r#","filesets":[{"path":"./some-provider","guestPath":"~/.some-provider"}]"#);
        let with_file = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            br#"{"token":"some_LNSPLACEHOLDER0000000000"}"#,
        )]);
        let other_file = MapFs::with(&[(
            "/work/some-provider/credentials.json",
            br#"{"token":"some_LNSPLACEHOLDER0000000000","extra":1}"#,
        )]);

        let one = read_local(
            &with_file,
            reading(doc.clone()),
            Path::new("/work/lns.yaml"),
        )
        .expect("one");
        let two = read_local(
            &other_file,
            reading(doc.clone()),
            Path::new("/work/lns.yaml"),
        )
        .expect("two");

        assert_ne!(
            one.digest, two.digest,
            "a fileset's content has to reach the digest a grant binds to"
        );
    }

    #[test]
    fn a_missing_path_fileset_names_the_method_and_the_path() {
        let doc = document(r#","filesets":[{"path":"./absent","guestPath":"~/.x"}]"#);
        let err = read_local(&MapFs::default(), reading(doc), Path::new("/work/lns.yaml"))
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
                Path::new("/work/lns.yaml")
            )
            .is_err()
        );
    }

    #[test]
    fn a_document_that_cannot_be_read_surfaces_that_rather_than_building() {
        let err = read_local(
            &MapFs::default(),
            |path| anyhow::bail!("reading {}", path.display()),
            Path::new("/work/lns.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("/work/lns.yaml"), "{err}");
    }
}
