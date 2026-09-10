use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::upper::{Change, ChangeSet};

/// The OCI whiteout prefix `changeset::classify_path` reads on the way back in.
const WHITEOUT_PREFIX: &str = ".wh.";

pub(crate) const LAYER_MEDIA_TYPE: &str = oci_client::manifest::IMAGE_LAYER_MEDIA_TYPE;

/// One uncompressed OCI layer: the tar bytes and, because uncompressed layers hash the same either way, one digest that is both the blob digest and the diff_id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerBlob {
    pub bytes: Vec<u8>,
    pub digest: String,
    pub entries: usize,
}

impl LayerBlob {
    pub(crate) fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

pub(crate) fn tar_layer(changes: &ChangeSet) -> Result<LayerBlob> {
    let mut bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut bytes);
        for change in &changes.changes {
            append(&mut builder, change)
                .with_context(|| format!("writing {} into the layer tar", change.path()))?;
        }
        builder.finish().context("closing the layer tar")?;
    }
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    Ok(LayerBlob {
        bytes,
        digest,
        entries: changes.changes.len(),
    })
}

/// `append_data` and `append_link` emit the GNU long-name entry a path or target past the header's fields needs; `set_path` would refuse one instead.
fn append<W: std::io::Write>(builder: &mut tar::Builder<W>, change: &Change) -> Result<()> {
    match change {
        Change::Directory {
            path,
            mode,
            uid,
            gid,
        } => {
            let mut header = header(*mode, *uid, *gid, 0, tar::EntryType::Directory);
            builder.append_data(&mut header, format!("{path}/"), std::io::empty())?;
        }
        Change::Regular {
            path,
            mode,
            uid,
            gid,
            bytes,
        } => {
            let mut header = header(
                *mode,
                *uid,
                *gid,
                bytes.len() as u64,
                tar::EntryType::Regular,
            );
            builder.append_data(&mut header, path, std::io::Cursor::new(bytes))?;
        }
        Change::Symlink {
            path,
            target,
            uid,
            gid,
        } => {
            let mut header = header(0o777, *uid, *gid, 0, tar::EntryType::Symlink);
            builder.append_link(&mut header, path, target)?;
        }
        Change::Removed { path } => {
            let mut header = header(0o644, 0, 0, 0, tar::EntryType::Regular);
            builder.append_data(&mut header, whiteout_path(path), std::io::empty())?;
        }
    }
    Ok(())
}

fn whiteout_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, name)) => format!("{parent}/{WHITEOUT_PREFIX}{name}"),
        None => format!("{WHITEOUT_PREFIX}{path}"),
    }
}

/// Every entry gets mtime 0: the ext4 upper carries no timestamp this reader can see, and a fixed one keeps one change set hashing to one layer digest.
fn header(mode: u32, uid: u32, gid: u32, size: u64, kind: tar::EntryType) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_mode(mode & 0o7777);
    header.set_uid(uid as u64);
    header.set_gid(gid as u64);
    header.set_mtime(0);
    header.set_size(size);
    header.set_entry_type(kind);
    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composefs::changeset::{PathChange, classify_path};
    use std::io::Read;

    fn change_set(changes: Vec<Change>) -> ChangeSet {
        ChangeSet { changes }
    }

    struct Entry {
        path: String,
        kind: tar::EntryType,
        mode: u32,
        uid: u64,
        gid: u64,
        link: Option<String>,
        body: Vec<u8>,
    }

    fn entries_of(layer: &LayerBlob) -> Vec<Entry> {
        let mut archive = tar::Archive::new(std::io::Cursor::new(&layer.bytes));
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let header = entry.header().clone();
                // `Entry`, not `Header`: the entry resolves the GNU long-name and long-link entries the header field truncates.
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let link = entry
                    .link_name()
                    .unwrap()
                    .map(|p| p.to_string_lossy().into_owned());
                let mut body = Vec::new();
                entry.read_to_end(&mut body).unwrap();
                Entry {
                    path,
                    kind: header.entry_type(),
                    mode: header.mode().unwrap(),
                    uid: header.uid().unwrap(),
                    gid: header.gid().unwrap(),
                    link,
                    body,
                }
            })
            .collect()
    }

    #[test]
    fn a_deletion_becomes_the_oci_whiteout_the_ingest_path_understands() {
        let layer = tar_layer(&change_set(vec![Change::Removed {
            path: "etc/alpine-release".into(),
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "etc/.wh.alpine-release");
        assert_eq!(entries[0].kind, tar::EntryType::Regular);
        assert!(entries[0].body.is_empty());
        assert_eq!(
            classify_path(std::path::Path::new(&entries[0].path)).unwrap(),
            Some(PathChange::Remove("etc/alpine-release".into())),
            "the whiteout must read back as the removal it stands for",
        );
    }

    #[test]
    fn a_deletion_at_the_root_is_whited_out_without_a_leading_slash() {
        let layer = tar_layer(&change_set(vec![Change::Removed {
            path: "toplevel".into(),
        }]))
        .unwrap();

        assert_eq!(entries_of(&layer)[0].path, ".wh.toplevel");
    }

    #[test]
    fn a_created_file_carries_its_bytes_mode_and_ownership() {
        let layer = tar_layer(&change_set(vec![Change::Regular {
            path: "spike-created".into(),
            mode: 0o640,
            uid: 1000,
            gid: 1001,
            bytes: b"built-by-lns\n".to_vec(),
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries[0].path, "spike-created");
        assert_eq!(entries[0].kind, tar::EntryType::Regular);
        assert_eq!(entries[0].mode, 0o640);
        assert_eq!(entries[0].uid, 1000);
        assert_eq!(entries[0].gid, 1001);
        assert_eq!(entries[0].body, b"built-by-lns\n");
    }

    #[test]
    fn a_directory_entry_ends_in_a_slash_so_tar_readers_call_it_one() {
        let layer = tar_layer(&change_set(vec![Change::Directory {
            path: "opt/tool".into(),
            mode: 0o750,
            uid: 0,
            gid: 0,
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries[0].path, "opt/tool/");
        assert_eq!(entries[0].kind, tar::EntryType::Directory);
        assert_eq!(entries[0].mode, 0o750);
    }

    #[test]
    fn a_symlink_entry_carries_its_target() {
        let layer = tar_layer(&change_set(vec![Change::Symlink {
            path: "bin/sh".into(),
            target: "busybox".into(),
            uid: 0,
            gid: 0,
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries[0].kind, tar::EntryType::Symlink);
        assert_eq!(entries[0].link.as_deref(), Some("busybox"));
    }

    #[test]
    fn a_mode_outside_the_permission_bits_is_masked_off() {
        let layer = tar_layer(&change_set(vec![Change::Regular {
            path: "f".into(),
            mode: 0o100_644,
            uid: 0,
            gid: 0,
            bytes: Vec::new(),
        }]))
        .unwrap();

        assert_eq!(entries_of(&layer)[0].mode, 0o644);
    }

    #[test]
    fn the_layer_digest_is_the_sha256_of_the_tar_and_the_entry_count_is_the_change_count() {
        let layer = tar_layer(&change_set(vec![
            Change::Directory {
                path: "etc".into(),
                mode: 0o755,
                uid: 0,
                gid: 0,
            },
            Change::Removed {
                path: "etc/alpine-release".into(),
            },
        ]))
        .unwrap();

        assert_eq!(
            layer.digest,
            format!("sha256:{}", hex::encode(Sha256::digest(&layer.bytes)))
        );
        assert_eq!(layer.entries, 2);
        assert_eq!(layer.size(), layer.bytes.len() as u64);
    }

    #[test]
    fn one_change_set_always_produces_one_digest() {
        let changes = || {
            change_set(vec![Change::Regular {
                path: "f".into(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                bytes: b"same".to_vec(),
            }])
        };
        assert_eq!(
            tar_layer(&changes()).unwrap().digest,
            tar_layer(&changes()).unwrap().digest,
        );
    }

    #[test]
    fn a_path_no_tar_header_can_hold_names_what_it_was_writing() {
        let err = tar_layer(&change_set(vec![Change::Regular {
            path: "bad\0path".into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: Vec::new(),
        }]))
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("writing bad\0path into the layer tar"),
            "{err:#}"
        );
    }

    /// `RUN npm install -g @anthropic-ai/claude-code`, #393's motivating example, leaves paths far
    /// past the 100-byte tar name field; a capture that refuses them refuses the whole run.
    #[test]
    fn a_path_past_the_tar_name_field_is_carried_by_a_gnu_long_name_entry() {
        let long = long_path();
        assert!(long.len() > 100, "the path must overflow the name field");

        let layer = tar_layer(&change_set(vec![Change::Regular {
            path: long.clone(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: b"module.exports = {}\n".to_vec(),
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries.len(), 1, "the long name must not become an entry");
        assert_eq!(entries[0].path, long);
        assert_eq!(entries[0].body, b"module.exports = {}\n");
    }

    #[test]
    fn a_symlink_target_past_the_link_field_keeps_the_target_it_pointed_at() {
        let target = long_path();
        assert!(
            target.len() > 100,
            "the target must overflow the link field"
        );

        let layer = tar_layer(&change_set(vec![Change::Symlink {
            path: "usr/local/bin/claude".into(),
            target: target.clone(),
            uid: 0,
            gid: 0,
        }]))
        .unwrap();

        let entries = entries_of(&layer);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "usr/local/bin/claude");
        assert_eq!(entries[0].link.as_deref(), Some(target.as_str()));
    }

    #[test]
    fn a_deleted_file_whose_path_is_long_still_reads_back_as_the_removal() {
        let long = format!("{}/gone", long_path());
        let layer = tar_layer(&change_set(vec![Change::Removed { path: long.clone() }])).unwrap();

        let entries = entries_of(&layer);
        assert_eq!(
            classify_path(std::path::Path::new(&entries[0].path)).unwrap(),
            Some(PathChange::Remove(long.into())),
        );
    }

    /// 150 bytes, shaped like the node_modules trees a real `RUN` leaves behind.
    fn long_path() -> String {
        let nested = "node_modules/@anthropic-ai/claude-code/".repeat(3);
        let path = format!("usr/local/lib/{nested}sharp-libvipsx.node");
        assert_eq!(path.len(), 150, "the fixture must be 150 bytes: {path}");
        path
    }

    #[test]
    fn an_empty_change_set_is_still_a_readable_tar() {
        let layer = tar_layer(&change_set(Vec::new())).unwrap();
        assert!(entries_of(&layer).is_empty());
        assert_eq!(layer.entries, 0);
    }
}
