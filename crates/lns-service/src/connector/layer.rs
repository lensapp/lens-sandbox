//! A connector's packed fileset, read back into the files a grant sends (`docs/sandbox-spec.md` §3.2.3).
//!
//! Distinct from `artifact::fileset`, which expands a sandbox's layer into the
//! runtime layer at boot: a connector's files travel in the policy frame, so
//! what is wanted here is bytes, not host paths.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Component;

use anyhow::{Context, Result, bail};

/// One packed fileset's files, keyed by the path they take under the entry's `guestPath`.
pub type PackedFiles = BTreeMap<String, PackedFile>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFile {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

/// The one refusal a caller may want to re-phrase: its own ceiling may be narrower than the one the method allows.
#[derive(Debug)]
pub struct BudgetExceeded(pub u64);

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "this fileset holds more than the {}-byte limit", self.0)
    }
}

impl std::error::Error for BudgetExceeded {}

/// Read one packed fileset layer, fail-closed on every entry a connector may not carry and on more bytes or entries than a push of the same directory could have produced.
pub fn expand(layer: &[u8], max_bytes: u64) -> Result<PackedFiles> {
    let mut files: PackedFiles = BTreeMap::new();
    let mut spent = 0u64;
    let gz = flate2::read::GzDecoder::new(layer);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("reading the fileset layer")? {
        let entry = entry.context("reading a fileset entry")?;
        let header = entry.header().clone();
        if header.entry_type().is_dir() {
            continue;
        }
        let path = entry
            .path()
            .context("reading a fileset entry path")?
            .into_owned();
        let name = path.to_string_lossy().into_owned();
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            bail!("fileset entry {name} escapes the path its method writes");
        }
        if name.chars().any(char::is_control) {
            bail!("fileset entry {name:?} must not contain control characters");
        }
        if !header.entry_type().is_file() {
            bail!("fileset entry {name} is not a regular file");
        }
        if files.len() == lns_artifact::build::MAX_FILESET_ENTRIES {
            bail!(
                "this fileset holds more than the {}-file limit",
                lns_artifact::build::MAX_FILESET_ENTRIES
            );
        }
        // Charged against what is read, never against a declared size: a PAX record overrides the header field, so the claim and the bytes are not the same number.
        let mut bytes = Vec::new();
        let remaining = max_bytes - spent;
        entry
            .take(remaining + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading fileset entry {name}"))?;
        if bytes.len() as u64 > remaining {
            return Err(anyhow::Error::new(BudgetExceeded(max_bytes)));
        }
        spent += bytes.len() as u64;
        files.insert(
            name,
            PackedFile {
                bytes,
                mode: header.mode().unwrap_or(0o600) & 0o777,
            },
        );
    }
    Ok(files)
}

/// Hand-built tar layers, because `tar::Builder` refuses most of what this module exists to refuse.
#[cfg(test)]
pub(crate) mod fixtures {
    /// One tar entry built by hand, because `tar::Builder` refuses most of what this module exists to refuse.
    pub fn raw_entry(name: &str, kind: u8, declared: u64, data: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{declared:011o}\0").as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len() + (512 - data.len() % 512) % 512, 0);
        out
    }

    pub fn gzipped(tar: Vec<u8>) -> Vec<u8> {
        use std::io::Write;
        let mut tar = tar;
        tar.resize(tar.len() + 1024, 0);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// `<len> <key>=<value>\n`, where `len` counts itself — the shape tar reads an extended header in.
    pub fn pax_record(key: &str, value: &str) -> Vec<u8> {
        let body = format!(" {key}={value}\n");
        let mut len = body.len() + 1;
        while format!("{len}").len() + body.len() != len {
            len += 1;
        }
        format!("{len}{body}").into_bytes()
    }

    pub fn raw_layer(name: &str, data: &[u8]) -> Vec<u8> {
        gzipped(raw_entry(name, b'0', data.len() as u64, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixtures::{gzipped, pax_record, raw_entry, raw_layer};

    fn packed(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let files: Vec<lns_artifact::build::FileEntry> = entries
            .iter()
            .map(|(path, data, mode)| lns_artifact::build::FileEntry {
                path: (*path).to_string(),
                data: data.to_vec(),
                mode: *mode,
            })
            .collect();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"seed","spec":{"filesets":[{"path":"./seed","guestPath":"/seed"}]}}"#;
        lns_artifact::build::build_artifact(doc, &[files], None)
            .expect("a packable directory")
            .fileset_layers()
            .next()
            .expect("one layer")
            .data
            .clone()
    }

    #[test]
    fn a_packed_fileset_reads_back_the_bytes_and_modes_it_was_packed_with() {
        let files = expand(
            &packed(&[
                ("config.json", b"{}", 0o600),
                ("bin/run", b"\x7fELF binary", 0o755),
            ]),
            1024,
        )
        .expect("the layer a push of that directory produced");

        assert_eq!(files["config.json"].bytes, b"{}");
        assert_eq!(files["config.json"].mode, 0o600);
        assert_eq!(
            files["bin/run"].bytes, b"\x7fELF binary",
            "a packed file is arbitrary bytes, so the read must not go through text"
        );
        assert_eq!(
            files["bin/run"].mode, 0o755,
            "a mode the author packed is the mode the guest gets: a file the workload must execute is useless at 0600"
        );
    }

    #[test]
    fn a_fileset_beyond_its_methods_ceiling_is_refused() {
        let err = expand(&packed(&[("config.json", &[b'a'; 2048], 0o600)]), 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("1024-byte limit"),
            "the ceiling is the caller's, because it belongs to the method that declares the fileset; got: {err:#}"
        );
    }

    #[test]
    fn the_ceiling_counts_every_entry_not_each_one_alone() {
        let err = expand(
            &packed(&[
                ("a.json", &[b'a'; 600], 0o600),
                ("b.json", &[b'b'; 600], 0o600),
            ]),
            1024,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("1024-byte limit"),
            "each entry clears the ceiling alone, so counting them apart would let a layer send twice what a method may; got: {err:#}"
        );
    }

    #[test]
    fn a_size_the_header_understates_is_charged_at_what_it_reads() {
        // A PAX record overrides the header's size field, so a budget charged against the field is charged against a number the entry does not honour.
        let body = vec![b'a'; 8192];
        let pax = pax_record("size", "8192");
        let mut tar = raw_entry("PaxHeaders/config.json", b'x', pax.len() as u64, &pax);
        tar.extend_from_slice(&raw_entry("config.json", b'0', 0, &body));

        let err = expand(&gzipped(tar), 1024).unwrap_err();

        assert!(
            format!("{err:#}").contains("1024-byte limit"),
            "a declared size is a claim, and a ceiling that trusts it bounds nothing; got: {err:#}"
        );
    }

    #[test]
    fn a_layer_holding_more_files_than_a_push_could_pack_is_refused() {
        // Zero-length entries cost no bytes, so a byte ceiling alone lets a layer carry files a push of the same directory would refuse — and each one becomes a guest file.
        let mut tar = Vec::new();
        for i in 0..lns_artifact::build::MAX_FILESET_ENTRIES + 1 {
            tar.extend_from_slice(&raw_entry(&format!("f{i}"), b'0', 0, b""));
        }

        let err = expand(&gzipped(tar), 1024).unwrap_err();

        assert!(format!("{err:#}").contains("file limit"), "got: {err:#}");
    }

    #[test]
    fn an_entry_that_escapes_the_path_its_method_writes_is_refused() {
        let err = expand(&raw_layer("../escape", b"x"), 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("escapes the path"),
            "a fileset writes under the guestPath its method disclosed, and nowhere else; got: {err:#}"
        );
    }

    #[test]
    fn an_entry_whose_name_carries_a_control_character_is_refused() {
        let err = expand(&raw_layer("con\nfig", b"x"), 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("control characters"),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_entry_that_is_not_a_regular_file_is_refused() {
        let err = expand(&gzipped(raw_entry("link", b'2', 0, b"")), 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a regular file"),
            "a symlink in a layer names a path the guest resolves, not one the document disclosed; got: {err:#}"
        );
    }

    #[test]
    fn a_directory_entry_is_not_a_file_and_is_not_refused_for_it() {
        let mut tar = raw_entry("nested/", b'5', 0, b"");
        tar.extend_from_slice(&raw_entry("nested/config.json", b'0', 2, b"{}"));

        let files =
            expand(&gzipped(tar), 1024).expect("a packed directory carries its directories");

        assert_eq!(
            files.keys().collect::<Vec<_>>(),
            ["nested/config.json"],
            "a directory entry makes no file, so it is skipped rather than written or refused"
        );
    }

    #[test]
    fn bytes_that_are_not_a_layer_are_refused_rather_than_read_as_empty() {
        let err = expand(b"not a gzipped tar", 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("fileset"),
            "an unreadable layer must not look like a fileset that writes nothing; got: {err:#}"
        );
    }
}
