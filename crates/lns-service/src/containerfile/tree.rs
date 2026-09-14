//! What the image a build stands on already holds at a path: `COPY entrypoint.sh /usr/local/bin`
//! writes into that directory when the image has one there, and a file at that path when it has not.

use anyhow::{Context, Result};

/// The OCI whiteout prefix a layer deletes an entry of the layer below it with.
const WHITEOUT_PREFIX: &str = ".wh.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentEntry {
    Directory,
    Other,
    Absent,
}

/// The image's layer blobs, newest last, as the local layer cache serves them.
pub(crate) trait LayerBytes {
    fn read(&self, digest: &str) -> Result<Vec<u8>>;
}

/// The topmost layer that says anything about the path is the one that decides it.
pub(crate) fn entry_at(
    layers: &dyn LayerBytes,
    digests: &[String],
    path: &str,
) -> Result<ParentEntry> {
    let wanted = path.trim_matches('/');
    if wanted.is_empty() {
        return Ok(ParentEntry::Directory);
    }
    for digest in digests.iter().rev() {
        let bytes = layers
            .read(digest)
            .with_context(|| format!("reading the layer {digest} of the image a COPY lands on"))?;
        if let Some(entry) = in_layer(&bytes, wanted)
            .with_context(|| format!("reading the layer {digest} of the image a COPY lands on"))?
        {
            return Ok(entry);
        }
    }
    Ok(ParentEntry::Absent)
}

fn in_layer(bytes: &[u8], wanted: &str) -> Result<Option<ParentEntry>> {
    let deleted = whiteout_of(wanted);
    let mut archive = open(bytes);
    for entry in archive.entries().context("reading the layer tar")? {
        let entry = entry.context("reading a layer entry")?;
        let path = entry.path().context("reading a layer entry's path")?;
        let name = normalized(&path.to_string_lossy());
        if name == deleted {
            return Ok(Some(ParentEntry::Absent));
        }
        if name == wanted {
            return Ok(Some(match entry.header().entry_type().is_dir() {
                true => ParentEntry::Directory,
                false => ParentEntry::Other,
            }));
        }
    }
    Ok(None)
}

fn open(bytes: &[u8]) -> tar::Archive<Box<dyn std::io::Read + '_>> {
    let reader: Box<dyn std::io::Read> = match bytes.starts_with(&[0x1f, 0x8b]) {
        true => Box::new(flate2::read::GzDecoder::new(std::io::Cursor::new(bytes))),
        false => Box::new(std::io::Cursor::new(bytes)),
    };
    tar::Archive::new(reader)
}

fn normalized(path: &str) -> String {
    path.trim_start_matches("./").trim_end_matches('/').into()
}

fn whiteout_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, name)) => format!("{parent}/{WHITEOUT_PREFIX}{name}"),
        None => format!("{WHITEOUT_PREFIX}{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeLayers {
        blobs: HashMap<String, Vec<u8>>,
    }

    impl FakeLayers {
        fn with(mut self, digest: &str, entries: &[(&str, tar::EntryType)]) -> Self {
            let mut bytes = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut bytes);
                for (path, kind) in entries {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_entry_type(*kind);
                    builder
                        .append_data(&mut header, path, std::io::empty())
                        .unwrap();
                }
                builder.finish().unwrap();
            }
            self.blobs.insert(digest.to_string(), bytes);
            self
        }
    }

    impl LayerBytes for FakeLayers {
        fn read(&self, digest: &str) -> Result<Vec<u8>> {
            self.blobs
                .get(digest)
                .cloned()
                .with_context(|| format!("no layer {digest} in the cache"))
        }
    }

    fn dir(path: &str) -> (&str, tar::EntryType) {
        (path, tar::EntryType::Directory)
    }

    fn file(path: &str) -> (&str, tar::EntryType) {
        (path, tar::EntryType::Regular)
    }

    #[test]
    fn a_directory_the_image_ships_is_a_directory() {
        let layers = FakeLayers::default().with("sha256:one", &[dir("./usr/local/bin/")]);

        assert_eq!(
            entry_at(&layers, &["sha256:one".into()], "/usr/local/bin").unwrap(),
            ParentEntry::Directory,
        );
    }

    #[test]
    fn a_file_the_image_ships_is_not_a_directory() {
        let layers = FakeLayers::default().with("sha256:one", &[file("usr/local/bin")]);

        assert_eq!(
            entry_at(&layers, &["sha256:one".into()], "/usr/local/bin").unwrap(),
            ParentEntry::Other,
        );
    }

    #[test]
    fn a_path_no_layer_mentions_is_absent() {
        let layers = FakeLayers::default().with("sha256:one", &[dir("usr")]);

        assert_eq!(
            entry_at(&layers, &["sha256:one".into()], "/srv/app").unwrap(),
            ParentEntry::Absent,
        );
    }

    #[test]
    fn the_topmost_layer_that_mentions_the_path_decides_it() {
        let layers = FakeLayers::default()
            .with("sha256:one", &[file("srv/app")])
            .with("sha256:two", &[dir("srv/app")]);

        assert_eq!(
            entry_at(
                &layers,
                &["sha256:one".into(), "sha256:two".into()],
                "/srv/app"
            )
            .unwrap(),
            ParentEntry::Directory,
        );
    }

    #[test]
    fn a_layer_that_deleted_the_path_leaves_it_absent() {
        let layers = FakeLayers::default()
            .with("sha256:one", &[dir("srv/app")])
            .with("sha256:two", &[file("srv/.wh.app")]);

        assert_eq!(
            entry_at(
                &layers,
                &["sha256:one".into(), "sha256:two".into()],
                "/srv/app"
            )
            .unwrap(),
            ParentEntry::Absent,
        );
    }

    #[test]
    fn a_deleted_entry_at_the_root_is_absent_too() {
        let layers = FakeLayers::default()
            .with("sha256:one", &[dir("srv")])
            .with("sha256:two", &[file(".wh.srv")]);

        assert_eq!(
            entry_at(&layers, &["sha256:one".into(), "sha256:two".into()], "/srv").unwrap(),
            ParentEntry::Absent,
        );
    }

    #[test]
    fn the_image_root_is_a_directory_without_reading_a_layer() {
        let layers = FakeLayers::default();

        assert_eq!(
            entry_at(&layers, &["sha256:missing".into()], "/").unwrap(),
            ParentEntry::Directory,
        );
    }

    #[test]
    fn a_gzipped_layer_reads_the_same_as_a_plain_one() {
        let plain = FakeLayers::default().with("sha256:one", &[dir("usr/local/bin")]);
        let mut zipped = FakeLayers::default();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &plain.blobs["sha256:one"]).unwrap();
        zipped
            .blobs
            .insert("sha256:one".into(), encoder.finish().unwrap());

        assert_eq!(
            entry_at(&zipped, &["sha256:one".into()], "/usr/local/bin").unwrap(),
            ParentEntry::Directory,
        );
    }

    #[test]
    fn a_layer_the_cache_cannot_serve_names_the_layer() {
        let layers = FakeLayers::default();

        let err = entry_at(&layers, &["sha256:gone".into()], "/srv").unwrap_err();

        assert!(format!("{err:#}").contains("sha256:gone"), "{err:#}");
    }

    #[test]
    fn bytes_that_are_not_a_tar_name_the_layer_they_came_from() {
        let mut layers = FakeLayers::default();
        layers.blobs.insert("sha256:junk".into(), vec![7; 2048]);

        let err = entry_at(&layers, &["sha256:junk".into()], "/srv").unwrap_err();

        assert!(format!("{err:#}").contains("sha256:junk"), "{err:#}");
    }
}
