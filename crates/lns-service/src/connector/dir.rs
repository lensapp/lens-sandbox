//! `~/.lns/connectors/<name>/` — one directory per installed connector, holding
//! `document.json` verbatim, the `digest` those bytes came from, and the packed
//! filesets the same artifact carried (`docs/cli-spec.md` §7.3).
//!
//! The layers are written before the document, so a write that fails part-way
//! leaves a connector whose grant cannot read what it needs rather than one
//! that reads the wrong bytes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lns_policy::secure_file::write_json_secret_atomic;

use super::store::{Installed, InstalledSet};

const DOCUMENT: &str = "document.json";
const DIGEST: &str = "digest";
const FILESETS: &str = "filesets";

pub struct ConnectorDir {
    root: PathBuf,
}

impl ConnectorDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn of(&self, name: &str) -> io::Result<PathBuf> {
        one_path_component(name)?;
        Ok(self.root.join(name))
    }
}

/// A name reaches the filesystem as a path component, so anything that could climb out of the root is refused before it is joined.
fn one_path_component(name: &str) -> io::Result<()> {
    let mut parts = Path::new(name).components();
    let single = matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if single && !name.starts_with('.') {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{name:?} is not a connector name this machine can store"),
    ))
}

impl InstalledSet for ConnectorDir {
    /// A directory missing its document is a half-finished install, not a fault, so it is skipped rather than failing every read.
    fn list(&self) -> io::Result<Vec<Installed>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut installed: Vec<Installed> = entries
            .flatten()
            .filter_map(|entry| read_one(&entry.path(), entry.file_name().to_str()?))
            .collect();
        installed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(installed)
    }

    fn put(
        &self,
        name: &str,
        digest: &str,
        document: &[u8],
        filesets: &[Vec<u8>],
    ) -> io::Result<()> {
        let dir = self.of(name)?;
        // The old layers go first: a reinstall that dropped a fileset would otherwise keep sending the files it no longer declares.
        match fs::remove_dir_all(dir.join(FILESETS)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        for (index, layer) in filesets.iter().enumerate() {
            write_json_secret_atomic(&layer_path(&dir, index), layer)?;
        }
        write_json_secret_atomic(&dir.join(DOCUMENT), document)?;
        write_json_secret_atomic(&dir.join(DIGEST), digest.as_bytes())
    }

    /// One packed fileset by its index in the document's `path` entries, which is the order it was kept in.
    fn fileset_layer(&self, name: &str, index: usize) -> io::Result<Vec<u8>> {
        fs::read(layer_path(&self.of(name)?, index))
    }

    fn remove(&self, name: &str) -> io::Result<bool> {
        match fs::remove_dir_all(self.of(name)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

fn layer_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(FILESETS).join(format!("{index}.tar.gz"))
}

fn read_one(dir: &Path, name: &str) -> Option<Installed> {
    Some(Installed {
        name: name.to_string(),
        digest: fs::read_to_string(dir.join(DIGEST))
            .ok()?
            .trim()
            .to_string(),
        document: fs::read(dir.join(DOCUMENT)).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> (tempfile::TempDir, ConnectorDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let set = ConnectorDir::new(tmp.path().join("connectors"));
        (tmp, set)
    }

    #[test]
    fn a_packed_fileset_is_kept_beside_the_document_it_came_with() {
        // A grant sends the files on every policy change, so the bytes have to outlive the install that fetched them.
        let (_tmp, set) = dir();
        set.put(
            "some-provider",
            "sha256:abc",
            b"{}",
            &[b"first".to_vec(), b"second".to_vec()],
        )
        .unwrap();

        assert_eq!(set.fileset_layer("some-provider", 0).unwrap(), b"first");
        assert_eq!(
            set.fileset_layer("some-provider", 1).unwrap(),
            b"second",
            "layers are kept in declaration order, which is the order the document's path entries are read in"
        );
    }

    #[test]
    fn uninstalling_takes_the_packed_filesets_with_it() {
        let (_tmp, set) = dir();
        set.put("some-provider", "sha256:abc", b"{}", &[b"first".to_vec()])
            .unwrap();

        assert!(set.remove("some-provider").unwrap());

        assert!(
            set.fileset_layer("some-provider", 0).is_err(),
            "real content under a name nothing installed is what uninstall exists to remove"
        );
    }

    #[test]
    fn reinstalling_with_fewer_filesets_leaves_none_of_the_old_ones() {
        // An update that dropped a fileset would otherwise keep sending the old one's files.
        let (_tmp, set) = dir();
        set.put(
            "some-provider",
            "sha256:abc",
            b"{}",
            &[b"first".to_vec(), b"second".to_vec()],
        )
        .unwrap();

        set.put("some-provider", "sha256:def", b"{}", &[b"only".to_vec()])
            .unwrap();

        assert_eq!(set.fileset_layer("some-provider", 0).unwrap(), b"only");
        assert!(set.fileset_layer("some-provider", 1).is_err());
    }

    #[test]
    fn a_machine_with_no_connectors_installed_lists_none() {
        // The root is created by the first install, so its absence is the empty set and not an error.
        let (_tmp, set) = dir();
        assert_eq!(set.list().unwrap(), Vec::new());
    }

    #[test]
    fn an_installed_document_reads_back_byte_for_byte() {
        // A grant binds to these bytes, so anything that reformats them would invalidate every grant.
        let (_tmp, set) = dir();
        let document = b"{\"apiVersion\":\"lns.run/v1\",  \"kind\":\"connector\"}";
        set.put("some-provider", "sha256:abc", document, &[])
            .unwrap();
        assert_eq!(
            set.list().unwrap(),
            vec![Installed {
                name: "some-provider".to_string(),
                digest: "sha256:abc".to_string(),
                document: document.to_vec(),
            }]
        );
    }

    #[test]
    fn installing_the_same_name_replaces_the_document_and_its_digest() {
        let (_tmp, set) = dir();
        set.put("some-provider", "sha256:old", b"{\"v\":1}", &[])
            .unwrap();
        set.put("some-provider", "sha256:new", b"{\"v\":2}", &[])
            .unwrap();
        let installed = set.list().unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].digest, "sha256:new");
        assert_eq!(installed[0].document, b"{\"v\":2}");
    }

    #[test]
    fn the_list_is_ordered_by_name_so_output_does_not_shuffle() {
        let (_tmp, set) = dir();
        for name in ["other-provider", "some-provider", "a-provider"] {
            set.put(name, "sha256:abc", b"{}", &[]).unwrap();
        }
        let names: Vec<String> = set.list().unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["a-provider", "other-provider", "some-provider"]);
    }

    #[test]
    fn a_directory_with_no_document_is_a_half_finished_install_and_is_skipped() {
        let (_tmp, set) = dir();
        set.put("some-provider", "sha256:abc", b"{}", &[]).unwrap();
        fs::create_dir_all(set.root.join("interrupted")).unwrap();
        let names: Vec<String> = set.list().unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["some-provider"]);
    }

    #[test]
    fn a_directory_with_a_document_but_no_digest_is_skipped_too() {
        // Without the digest there is nothing for a grant to bind to, so the entry cannot be offered.
        let (_tmp, set) = dir();
        let half = set.root.join("interrupted");
        fs::create_dir_all(&half).unwrap();
        fs::write(half.join(DOCUMENT), b"{}").unwrap();
        assert_eq!(set.list().unwrap(), Vec::new());
    }

    #[test]
    fn uninstalling_reports_whether_anything_was_there() {
        let (_tmp, set) = dir();
        set.put("some-provider", "sha256:abc", b"{}", &[]).unwrap();
        assert!(set.remove("some-provider").unwrap());
        assert!(!set.remove("some-provider").unwrap());
        assert_eq!(set.list().unwrap(), Vec::new());
    }

    #[test]
    fn a_name_that_could_climb_out_of_the_root_is_refused() {
        // A traversing name would write a connector's document anywhere the service can reach.
        let (_tmp, set) = dir();
        for bad in ["../escaped", "a/b", "..", ".", "", ".hidden", "/absolute"] {
            let err = set
                .put(bad, "sha256:abc", b"{}", &[])
                .expect_err("a traversing name must be refused");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
    }

    #[test]
    fn a_connector_path_occupied_by_a_file_surfaces_the_error_rather_than_reporting_nothing() {
        // Reporting "nothing was there" would tell the user the connector is gone while its path still is.
        let (_tmp, set) = dir();
        fs::create_dir_all(&set.root).unwrap();
        fs::write(set.root.join("some-provider"), b"not a directory").unwrap();
        let err = set.remove("some-provider").unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound, "{err:?}");
    }

    #[test]
    fn a_traversing_name_is_refused_by_remove_as_well_as_put() {
        // Otherwise `uninstall` becomes a delete-any-directory primitive.
        let (_tmp, set) = dir();
        assert_eq!(
            set.remove("../escaped").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn a_root_occupied_by_a_file_surfaces_the_error_rather_than_reading_nothing() {
        // Reporting "nothing installed" here would silently re-offer connectors the machine actually holds.
        let tmp = tempfile::TempDir::new().unwrap();
        let occupied = tmp.path().join("connectors");
        fs::write(&occupied, b"not a directory").unwrap();
        let set = ConnectorDir::new(occupied);
        let err = set.list().unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound, "{err:?}");
    }

    #[test]
    fn an_install_that_cannot_land_surfaces_its_error() {
        let (_tmp, set) = dir();
        fs::create_dir_all(set.root.join("some-provider").join(DOCUMENT)).unwrap();
        assert!(set.put("some-provider", "sha256:abc", b"{}", &[]).is_err());
    }

    #[test]
    fn the_document_is_not_world_readable() {
        // A connector document is not itself a secret, but it sits in the same 0700 tree and the one write helper keeps one rule.
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, set) = dir();
        set.put("some-provider", "sha256:abc", b"{}", &[]).unwrap();
        let mode = fs::metadata(set.root.join("some-provider").join(DOCUMENT))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "got 0o{mode:o}");
    }
}
