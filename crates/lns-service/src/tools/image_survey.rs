use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;

use super::Libc;
use crate::composefs::changeset::{PathChange, classify_path};

/// What one walk of an image's layers settles about the rootfs a run will boot: which libc a provisioned tool has to be built against, and whether a login shell there reads `/etc/profile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSurvey {
    pub libc: Libc,
    pub reads_etc_profile: bool,
}

/// Gunzipping and walking every layer is CPU work on borrowed data, so it moves off the async worker rather than holding one for the length of the scan; a current-thread runtime (tests) has nowhere to move it and runs it inline.
pub fn survey_off_runtime(
    layer_digests: &[String],
    layers: &[impl AsRef<[u8]>],
) -> Result<ImageSurvey> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| survey_for(layer_digests, layers))
        }
        _ => survey_for(layer_digests, layers),
    }
}

/// Every fact here is a pure function of the layer set, so a warm run re-answers them from the digests instead of gunzipping every layer body again.
pub fn survey_for(layer_digests: &[String], layers: &[impl AsRef<[u8]>]) -> Result<ImageSurvey> {
    static MEMO: OnceLock<Mutex<HashMap<String, ImageSurvey>>> = OnceLock::new();
    // Without digests there is nothing identifying to key on, and every such image would share one entry.
    if layer_digests.is_empty() {
        return survey(layers);
    }
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    let key = layer_digests.join(" ");
    if let Some(hit) = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return Ok(*hit);
    }
    let surveyed = survey(layers)?;
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, surveyed);
    Ok(surveyed)
}

/// Decide the image's libc flavor and its login-shell entry point from what is visible after applying its layer changesets.
pub fn survey(layers: &[impl AsRef<[u8]>]) -> Result<ImageSurvey> {
    let mut visible: BTreeMap<PathBuf, Evidence> = BTreeMap::new();
    for (idx, layer) in layers.iter().enumerate() {
        let changes =
            scan_layer(layer.as_ref()).with_context(|| format!("scanning layer {idx}"))?;
        for target in changes.removals {
            visible.retain(|path, _| !path.starts_with(&target));
        }
        for entry in changes.entries {
            if entry.is_directory {
                visible.remove(&entry.path);
            } else {
                visible.retain(|path, _| !path.starts_with(&entry.path));
                if let Some(evidence) = evidence_of(&entry.path) {
                    visible.insert(entry.path, evidence);
                }
            }
        }
    }
    let holds = |wanted: Evidence| visible.values().any(|evidence| *evidence == wanted);
    let has_musl_loader = holds(Evidence::Loader(Libc::Musl));
    let has_gnu_loader = holds(Evidence::Loader(Libc::Gnu));
    Ok(ImageSurvey {
        libc: if has_musl_loader && (!has_gnu_loader || holds(Evidence::Gcompat)) {
            Libc::Musl
        } else {
            Libc::Gnu
        },
        reads_etc_profile: holds(Evidence::LoginProfile),
    })
}

struct LayerChanges {
    removals: Vec<PathBuf>,
    entries: Vec<LayerEntry>,
}

struct LayerEntry {
    path: PathBuf,
    is_directory: bool,
}

fn scan_layer(bytes: &[u8]) -> Result<LayerChanges> {
    let reader: Box<dyn Read> = if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    } else {
        Box::new(Cursor::new(bytes))
    };
    let mut archive = tar::Archive::new(reader);
    let mut removals = Vec::new();
    let mut entries = Vec::new();
    for entry in archive.entries().context("reading layer tar")? {
        let entry = entry.context("reading layer entry")?;
        let path = entry.path().context("reading layer entry path")?;
        match classify_path(path.as_ref())? {
            Some(PathChange::Entry(path)) if materializes(entry.header().entry_type()) => {
                entries.push(LayerEntry {
                    path,
                    is_directory: entry.header().entry_type().is_dir(),
                });
            }
            Some(PathChange::Remove(target) | PathChange::ClearDirectory(target)) => {
                removals.push(target);
            }
            Some(PathChange::Entry(_)) | None => {}
        }
    }
    Ok(LayerChanges { removals, entries })
}

fn materializes(entry_type: tar::EntryType) -> bool {
    entry_type.is_dir()
        || matches!(
            entry_type,
            tar::EntryType::Regular
                | tar::EntryType::Continuous
                | tar::EntryType::Symlink
                | tar::EntryType::Fifo
        )
}

const LOADER_DIRS: &[&str] = &["lib", "lib64", "usr/lib", "usr/lib64"];

/// The login-shell entry point: it is what sources `/etc/profile.d`, so an image without it reads no snippet the host writes there.
const LOGIN_PROFILE: &str = "etc/profile";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    Loader(Libc),
    Gcompat,
    LoginProfile,
}

fn evidence_of(path: &Path) -> Option<Evidence> {
    let rel = path.to_string_lossy();
    let rel = rel.trim_start_matches("./").trim_start_matches('/');
    if rel == LOGIN_PROFILE {
        return Some(Evidence::LoginProfile);
    }
    let (dir, name) = rel.rsplit_once('/')?;
    let dir = dir.trim_start_matches("./");
    if !LOADER_DIRS.contains(&dir) && !dir.starts_with("usr/lib/") {
        return None;
    }
    if name.starts_with("ld-musl-") {
        return Some(Evidence::Loader(Libc::Musl));
    }
    if name.starts_with("ld-linux") || name == "libc.so.6" {
        return Some(Evidence::Loader(Libc::Gnu));
    }
    if name.starts_with("libgcompat.so") {
        return Some(Evidence::Gcompat);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_with(paths: &[&str]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for path in paths {
            let body: &[u8] = if path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with(".wh."))
            {
                b""
            } else {
                b"x"
            };
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, body).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn layer_with_typed(path: &str, entry_type: tar::EntryType) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_entry_type(entry_type);
        if entry_type == tar::EntryType::Link {
            header.set_link_name("elsewhere").unwrap();
        }
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();
        builder.into_inner().unwrap()
    }

    #[test]
    fn a_musl_loader_marks_the_image_musl() {
        let layer = layer_with(&["bin/sh", "lib/ld-musl-aarch64.so.1"]);
        assert_eq!(survey(&[layer]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_gnu_loader_or_libc_marks_the_image_gnu() {
        let loader = layer_with(&["lib/ld-linux-aarch64.so.1"]);
        assert_eq!(survey(&[loader]).unwrap().libc, Libc::Gnu);
        let libc = layer_with(&["usr/lib/x86_64-linux-gnu/libc.so.6"]);
        assert_eq!(survey(&[libc]).unwrap().libc, Libc::Gnu);
    }

    #[test]
    fn a_loaderless_image_defaults_to_gnu() {
        // Every image fills lib/ with things that are not loaders.
        let layer = layer_with(&["app/server", "lib/libz.so.1", "usr/lib/libssl.so.3"]);
        assert_eq!(survey(&[layer]).unwrap().libc, Libc::Gnu);
        let empty: [Vec<u8>; 0] = [];
        assert_eq!(survey(&empty).unwrap().libc, Libc::Gnu);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_scan_moves_off_the_worker_on_a_multi_thread_runtime() {
        let digests = vec!["sha256:offruntime".to_string()];
        let layers = vec![layer_with(&["lib/ld-musl-aarch64.so.1"])];
        assert_eq!(
            survey_off_runtime(&digests, &layers).unwrap().libc,
            Libc::Musl
        );
    }

    #[tokio::test]
    async fn the_scan_runs_inline_where_there_is_nowhere_to_move_it() {
        let digests = vec!["sha256:inline".to_string()];
        let layers = vec![layer_with(&["lib/ld-linux-aarch64.so.1"])];
        assert_eq!(
            survey_off_runtime(&digests, &layers).unwrap().libc,
            Libc::Gnu
        );
    }

    #[test]
    fn a_second_run_on_the_same_layer_set_answers_from_the_digests_without_rescanning() {
        let digests = vec!["sha256:aaaa".to_string(), "sha256:bbbb".to_string()];
        let layers = vec![
            layer_with(&["bin/sh"]),
            layer_with(&["lib/ld-musl-x86_64.so.1"]),
        ];
        assert_eq!(survey_for(&digests, &layers).unwrap().libc, Libc::Musl);
        let corrupt = vec![b"\x1f\x8bnot really gzip".to_vec(), Vec::new()];
        assert_eq!(
            survey_for(&digests, &corrupt).unwrap().libc,
            Libc::Musl,
            "bodies that would fail to scan are never opened on a hit"
        );
    }

    #[test]
    fn an_undigested_layer_set_is_scanned_every_time_rather_than_sharing_one_entry() {
        let musl = vec![layer_with(&["lib/ld-musl-aarch64.so.1"])];
        assert_eq!(survey_for(&[], &musl).unwrap().libc, Libc::Musl);
        let gnu = vec![layer_with(&["lib/ld-linux-aarch64.so.1"])];
        assert_eq!(
            survey_for(&[], &gnu).unwrap().libc,
            Libc::Gnu,
            "a second undigested image gets its own verdict, not the first one's"
        );
    }

    #[test]
    fn a_musl_loader_does_not_hide_a_corrupt_later_entry() {
        let mut layer = layer_with(&["lib/ld-musl-aarch64.so.1", "usr/lib/libc.so.6"]);
        layer[1024] ^= 0xff;
        assert!(survey(&[layer]).is_err());
    }

    #[test]
    fn a_borrowed_layer_set_is_scanned_without_owning_the_bytes() {
        let owned = layer_with(&["lib/ld-musl-aarch64.so.1"]);
        let borrowed: Vec<&[u8]> = vec![owned.as_slice()];
        assert_eq!(survey(&borrowed).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_later_compat_shim_cannot_overturn_the_base_images_musl_loader() {
        // `alpine + apk add gcompat` adds a gnu loader without removing the musl one; reading that as glibc installs tool builds that cannot run.
        let alpine = layer_with(&["lib/ld-musl-x86_64.so.1", "bin/busybox"]);
        let gcompat = layer_with(&["lib/ld-linux-x86-64.so.2", "usr/lib/libgcompat.so.0"]);
        assert_eq!(survey(&[alpine, gcompat]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_later_whiteout_can_replace_a_lower_musl_loader_with_gnu() {
        let musl = layer_with(&["lib/ld-musl-x86_64.so.1"]);
        let gnu = layer_with(&["lib/.wh.ld-musl-x86_64.so.1", "lib64/ld-linux-x86-64.so.2"]);
        assert_eq!(survey(&[musl, gnu]).unwrap().libc, Libc::Gnu);
    }

    #[test]
    fn an_opaque_whiteout_preserves_a_same_layer_gnu_loader() {
        let musl = layer_with(&["lib/ld-musl-x86_64.so.1"]);
        let gnu = layer_with(&["lib/ld-linux-x86-64.so.2", "lib/.wh..wh..opq"]);
        assert_eq!(survey(&[musl, gnu]).unwrap().libc, Libc::Gnu);
    }

    #[test]
    fn a_later_layer_can_still_add_the_only_loader_in_the_image() {
        let scratch = layer_with(&["app/server"]);
        let runtime = layer_with(&["lib/ld-musl-aarch64.so.1"]);
        assert_eq!(survey(&[scratch, runtime]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_directory_replacing_a_loader_path_removes_that_loader() {
        let musl = layer_with(&["lib/ld-musl-x86_64.so.1"]);
        let replacement = layer_with_typed("lib/ld-musl-x86_64.so.1", tar::EntryType::Directory);
        assert_eq!(survey(&[musl, replacement]).unwrap().libc, Libc::Gnu);
    }

    #[test]
    fn an_entry_type_the_materializer_skips_does_not_hide_a_loader() {
        let musl = layer_with(&["lib/ld-musl-x86_64.so.1"]);
        let skipped = layer_with_typed("lib/ld-musl-x86_64.so.1", tar::EntryType::Link);
        assert_eq!(survey(&[musl, skipped]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_loader_name_outside_a_loader_directory_decides_nothing() {
        // A tool tree or source checkout that merely carries the name is not the image's loader.
        let layer = layer_with(&[
            "opt/toolchain/sysroot/ld-musl-x86_64.so.1",
            "src/musl/ld-linux-x86-64.so.2",
        ]);
        assert_eq!(survey(&[layer]).unwrap().libc, Libc::Gnu, "no loader found");
    }

    #[test]
    fn a_layer_carrying_both_loaders_and_gcompat_counts_as_musl() {
        let layer = layer_with(&[
            "lib/ld-musl-x86_64.so.1",
            "lib/ld-linux-x86-64.so.2",
            "lib/libgcompat.so.0",
        ]);
        assert_eq!(survey(&[layer]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn a_glibc_image_with_a_co_installed_musl_loader_stays_gnu() {
        let layer = layer_with(&[
            "lib64/ld-linux-x86-64.so.2",
            "usr/lib/x86_64-linux-gnu/libc.so.6",
            "lib/ld-musl-x86_64.so.1",
        ]);
        assert_eq!(survey(&[layer]).unwrap().libc, Libc::Gnu);
    }

    #[test]
    fn a_gzip_compressed_layer_is_sniffed_and_decompressed() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let raw = layer_with(&["lib/ld-musl-aarch64.so.1"]);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&raw).unwrap();
        let gz = encoder.finish().unwrap();
        assert_eq!(survey(&[gz]).unwrap().libc, Libc::Musl);
    }

    #[test]
    fn an_image_that_ships_etc_profile_is_reported_as_reading_it() {
        // /etc/profile is what sources /etc/profile.d, so it decides whether writing a snippet there puts a declared tool back on a login shell's PATH.
        let layer = layer_with(&["etc/profile", "lib/ld-musl-x86_64.so.1"]);
        assert!(survey(&[layer]).unwrap().reads_etc_profile);
    }

    #[test]
    fn an_image_with_no_etc_profile_is_reported_as_reading_none() {
        // A distroless or scratch rootfs has no login shell at all; a file written for one would never be read.
        let layer = layer_with(&["app/server", "etc/passwd", "etc/profile.d/other.sh"]);
        let surveyed = survey(&[layer]).unwrap();
        assert!(!surveyed.reads_etc_profile);
        assert_eq!(surveyed.libc, Libc::Gnu, "one walk still settles both");
    }

    #[test]
    fn a_later_layer_that_whites_out_etc_profile_takes_the_login_shell_with_it() {
        let base = layer_with(&["etc/profile"]);
        let stripped = layer_with(&["etc/.wh.profile"]);
        assert!(!survey(&[base, stripped]).unwrap().reads_etc_profile);
    }

    #[test]
    fn a_later_layer_can_add_the_login_shell_entry_point_the_base_lacked() {
        let base = layer_with(&["app/server"]);
        let added = layer_with(&["etc/profile"]);
        assert!(survey(&[base, added]).unwrap().reads_etc_profile);
    }

    #[test]
    fn the_memo_carries_every_fact_of_the_walk_rather_than_the_libc_alone() {
        // Both answers come from one gunzip of the layer bodies; a hit that dropped one would send the second reader back through the whole scan.
        let digests = vec!["sha256:cccc".to_string()];
        let layers = vec![layer_with(&["etc/profile", "lib/ld-musl-x86_64.so.1"])];
        assert_eq!(
            survey_for(&digests, &layers).unwrap(),
            ImageSurvey {
                libc: Libc::Musl,
                reads_etc_profile: true
            }
        );
        let corrupt = vec![b"\x1f\x8bnot really gzip".to_vec()];
        assert_eq!(
            survey_for(&digests, &corrupt).unwrap(),
            ImageSurvey {
                libc: Libc::Musl,
                reads_etc_profile: true
            }
        );
    }

    #[test]
    fn a_corrupt_layer_surfaces_the_layer_index() {
        let err = survey(&[b"\x1f\x8bnot really gzip".to_vec()]).unwrap_err();
        assert!(
            format!("{err:#}").contains("scanning layer 0"),
            "got: {err:#}"
        );
    }
}
