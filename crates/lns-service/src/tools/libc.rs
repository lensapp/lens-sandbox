use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;

use super::Libc;

/// The flavor is a pure function of the layer set, so a warm run re-answers it from the digests instead of gunzipping every layer body again.
pub fn detect_libc_for(layer_digests: &[String], layers: &[impl AsRef<[u8]>]) -> Result<Libc> {
    static MEMO: OnceLock<Mutex<HashMap<String, Libc>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    let key = layer_digests.join(" ");
    if let Some(hit) = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return Ok(*hit);
    }
    let verdict = detect_libc(layers)?;
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, verdict);
    Ok(verdict)
}

/// Decide the image's libc flavor from its layer tars so installed tool builds match the workload: the dynamic loader's file name is definitive, later layers override earlier ones (overlay semantics), and a static/distroless image with no loader defaults to gnu.
pub fn detect_libc(layers: &[impl AsRef<[u8]>]) -> Result<Libc> {
    let mut verdict = None;
    for (idx, layer) in layers.iter().enumerate() {
        if let Some(found) =
            scan_layer(layer.as_ref()).with_context(|| format!("scanning layer {idx}"))?
        {
            verdict = Some(found);
        }
    }
    Ok(verdict.unwrap_or(Libc::Gnu))
}

fn scan_layer(bytes: &[u8]) -> Result<Option<Libc>> {
    let reader: Box<dyn Read> = if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    } else {
        Box::new(Cursor::new(bytes))
    };
    let mut archive = tar::Archive::new(reader);
    let mut found = None;
    for entry in archive.entries().context("reading layer tar")? {
        let entry = entry.context("reading layer entry")?;
        let path = entry.path().context("reading layer entry path")?;
        let name = path.to_string_lossy().trim_start_matches("./").to_string();
        if name.contains("ld-musl-") {
            return Ok(Some(Libc::Musl));
        }
        if name.contains("ld-linux-") || name.ends_with("libc.so.6") {
            found = Some(Libc::Gnu);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_with(paths: &[&str]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for path in paths {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &b"x"[..]).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn a_musl_loader_marks_the_image_musl() {
        let layer = layer_with(&["bin/sh", "lib/ld-musl-aarch64.so.1"]);
        assert_eq!(detect_libc(&[layer]).unwrap(), Libc::Musl);
    }

    #[test]
    fn a_gnu_loader_or_libc_marks_the_image_gnu() {
        let loader = layer_with(&["lib/ld-linux-aarch64.so.1"]);
        assert_eq!(detect_libc(&[loader]).unwrap(), Libc::Gnu);
        let libc = layer_with(&["usr/lib/x86_64-linux-gnu/libc.so.6"]);
        assert_eq!(detect_libc(&[libc]).unwrap(), Libc::Gnu);
    }

    #[test]
    fn a_loaderless_image_defaults_to_gnu() {
        let layer = layer_with(&["app/server"]);
        assert_eq!(detect_libc(&[layer]).unwrap(), Libc::Gnu);
        let empty: [Vec<u8>; 0] = [];
        assert_eq!(detect_libc(&empty).unwrap(), Libc::Gnu);
    }

    #[test]
    fn a_second_run_on_the_same_layer_set_answers_from_the_digests_without_rescanning() {
        let digests = vec!["sha256:aaaa".to_string(), "sha256:bbbb".to_string()];
        let layers = vec![
            layer_with(&["bin/sh"]),
            layer_with(&["lib/ld-musl-x86_64.so.1"]),
        ];
        assert_eq!(detect_libc_for(&digests, &layers).unwrap(), Libc::Musl);
        let corrupt = vec![b"\x1f\x8bnot really gzip".to_vec(), Vec::new()];
        assert_eq!(
            detect_libc_for(&digests, &corrupt).unwrap(),
            Libc::Musl,
            "bodies that would fail to scan are never opened on a hit"
        );
    }

    #[test]
    fn a_musl_layer_stops_scanning_at_the_loader() {
        // Nothing later in the layer can change a musl verdict, so a big rootfs tar is not walked to the end.
        let mut builder = tar::Builder::new(Vec::new());
        for path in ["lib/ld-musl-aarch64.so.1", "usr/lib/libc.so.6"] {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(1);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &b"x"[..]).unwrap();
        }
        let mut layer = builder.into_inner().unwrap();
        layer.extend_from_slice(b"trailing garbage that would fail a full walk");
        assert_eq!(detect_libc(&[layer]).unwrap(), Libc::Musl);
    }

    #[test]
    fn a_borrowed_layer_set_is_scanned_without_owning_the_bytes() {
        let owned = layer_with(&["lib/ld-musl-aarch64.so.1"]);
        let borrowed: Vec<&[u8]> = vec![owned.as_slice()];
        assert_eq!(detect_libc(&borrowed).unwrap(), Libc::Musl);
    }

    #[test]
    fn a_later_layer_overrides_the_base_flavor() {
        let base = layer_with(&["lib/ld-linux-aarch64.so.1"]);
        let overlay = layer_with(&["lib/ld-musl-aarch64.so.1"]);
        assert_eq!(detect_libc(&[base, overlay]).unwrap(), Libc::Musl);
    }

    #[test]
    fn a_layer_carrying_both_markers_counts_as_musl() {
        let layer = layer_with(&["lib/ld-musl-x86_64.so.1", "opt/glibc/libc.so.6"]);
        assert_eq!(detect_libc(&[layer]).unwrap(), Libc::Musl);
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
        assert_eq!(detect_libc(&[gz]).unwrap(), Libc::Musl);
    }

    #[test]
    fn a_corrupt_layer_surfaces_the_layer_index() {
        let err = detect_libc(&[b"\x1f\x8bnot really gzip".to_vec()]).unwrap_err();
        assert!(
            format!("{err:#}").contains("scanning layer 0"),
            "got: {err:#}"
        );
    }
}
