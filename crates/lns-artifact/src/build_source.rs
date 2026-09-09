//! The Containerfile and its context, packed as one layer of the artifact the
//! document publishes as, so an approver reads what a build ran off the pulled
//! artifact rather than off the author's machine (`docs/sandbox-spec.md` §7.3).

use anyhow::{Context, Result, bail};

use crate::build::{Blob, FileEntry, MAX_FILESET_BYTES, MAX_FILESET_ENTRIES};

/// The media type of the layer a push packs a path-form `spec.image`'s Containerfile and context into (§7.3).
pub const BUILD_SOURCE_LAYER_MEDIA_TYPE: &str = "application/vnd.lns.image.source.v1.tar+gzip";

/// What a push packs: the Containerfile as `spec.image` named it, and every file the context sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSourceLayer {
    /// The Containerfile as the summary spells it, such as `./image/Containerfile`.
    pub containerfile: String,
    /// Every file of the context, its path relative to the context root.
    pub files: Vec<FileEntry>,
}

/// The build source as a consumer reads it back out of a pulled artifact (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSource {
    pub containerfile: String,
    pub text: String,
    pub files: Vec<SourceFile>,
}

/// One packed context file, as an approver reads it: where it sits and how much of the artifact it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub path: String,
    pub bytes: u64,
}

/// The name inside the context that holds the instructions, which is the last segment of the label.
fn packed_name(containerfile: &str) -> &str {
    containerfile.rsplit('/').next().unwrap_or(containerfile)
}

/// Pack the Containerfile and its context into the one layer the artifact carries them in.
pub fn pack(source: &ImageSourceLayer) -> Result<Blob> {
    let name = packed_name(&source.containerfile);
    if !source.files.iter().any(|file| file.path == name) {
        bail!(
            "the build context does not hold {name}, so there is nothing for a consumer to read the build off"
        );
    }
    let data = crate::build::pack_entries(&source.files, MAX_FILESET_BYTES, MAX_FILESET_ENTRIES)
        .context("packing the build source layer")?;
    Ok(Blob {
        digest: crate::build::digest_of(&data),
        media_type: BUILD_SOURCE_LAYER_MEDIA_TYPE.to_string(),
        data,
    })
}

/// Read a packed build source back: the Containerfile's text, and every context file with its size.
pub fn read(containerfile: &str, layer: &[u8]) -> Result<BuildSource> {
    read_within(containerfile, layer, MAX_FILESET_BYTES)
}

/// The read with its ceiling in hand, because what a layer expands to is the registry's claim until it is counted.
fn read_within(containerfile: &str, layer: &[u8], max_bytes: u64) -> Result<BuildSource> {
    let name = packed_name(containerfile);
    let mut files = Vec::new();
    let mut text = None;
    let mut total = 0_u64;
    let decoder = flate2::read::GzDecoder::new(layer);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .context("reading the build source layer")?
    {
        let mut entry = entry.context("reading the build source layer")?;
        let path = entry
            .path()
            .context("reading a build source path")?
            .to_string_lossy()
            .into_owned();
        let bytes = entry.size();
        total = total.saturating_add(bytes);
        if total > max_bytes {
            bail!("the build source layer expands past the {max_bytes}-byte limit");
        }
        if path == name {
            let mut read = String::new();
            std::io::Read::read_to_string(&mut entry, &mut read)
                .with_context(|| format!("reading {path} out of the build source layer"))?;
            text = Some(read);
        }
        files.push(SourceFile { path, bytes });
    }
    let Some(text) = text else {
        bail!("the build source layer carries no {name}");
    };
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(BuildSource {
        containerfile: containerfile.to_string(),
        text,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, data: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            data: data.as_bytes().to_vec(),
            mode: 0o644,
        }
    }

    fn source() -> ImageSourceLayer {
        ImageSourceLayer {
            containerfile: "./image/Containerfile".to_string(),
            files: vec![
                file("Containerfile", "FROM alpine\nCOPY app /srv\n"),
                file("app/main.js", "console.log(1)\n"),
            ],
        }
    }

    #[test]
    fn a_packed_source_reads_back_as_the_containerfile_and_the_files_beside_it() {
        let blob = pack(&source()).expect("packing");
        assert_eq!(blob.media_type, BUILD_SOURCE_LAYER_MEDIA_TYPE);

        let read = read("./image/Containerfile", &blob.data).expect("reading it back");
        assert_eq!(read.text, "FROM alpine\nCOPY app /srv\n");
        assert_eq!(
            read.files,
            vec![
                SourceFile {
                    path: "Containerfile".into(),
                    bytes: 26,
                },
                SourceFile {
                    path: "app/main.js".into(),
                    bytes: 15,
                },
            ],
            "an approver reads the same list off the artifact that inspect showed the author"
        );
    }

    #[test]
    fn a_dockerfile_under_its_own_name_reads_back_the_same_way() {
        let blob = pack(&ImageSourceLayer {
            containerfile: "./image/Dockerfile".into(),
            files: vec![file("Dockerfile", "FROM alpine\n")],
        })
        .expect("packing");
        assert_eq!(
            read("./image/Dockerfile", &blob.data)
                .expect("reading")
                .text,
            "FROM alpine\n"
        );
    }

    #[test]
    fn a_containerfile_named_by_a_bare_path_is_found_without_a_directory_in_front_of_it() {
        assert_eq!(packed_name("Containerfile"), "Containerfile");
    }

    #[test]
    fn packing_a_context_that_does_not_hold_the_containerfile_is_refused() {
        let err = pack(&ImageSourceLayer {
            containerfile: "./image/Containerfile".into(),
            files: vec![file("app/main.js", "x")],
        })
        .expect_err("a layer with no instructions in it discloses nothing")
        .to_string();
        assert!(err.contains("Containerfile"), "{err}");
    }

    #[test]
    fn a_packed_source_is_deterministic_so_two_pushes_of_one_context_dedupe() {
        assert_eq!(
            pack(&source()).unwrap().digest,
            pack(&source()).unwrap().digest
        );
    }

    #[test]
    fn a_layer_that_is_not_a_gzipped_tar_is_refused_rather_than_read_as_empty() {
        let err = read("./image/Containerfile", b"not a layer")
            .expect_err("a layer that does not decode discloses nothing")
            .to_string();
        assert!(err.contains("build source layer"), "{err}");
    }

    #[test]
    fn a_layer_missing_the_containerfile_is_refused_naming_the_file_it_wanted() {
        let blob = crate::build::pack_entries(
            &[file("app/main.js", "x")],
            MAX_FILESET_BYTES,
            MAX_FILESET_ENTRIES,
        )
        .unwrap();
        let err = read("./image/Containerfile", &blob)
            .expect_err("the instructions are the point of the layer")
            .to_string();
        assert!(err.contains("carries no Containerfile"), "{err}");
    }

    #[test]
    fn a_layer_that_expands_past_the_limit_is_refused_before_it_is_all_read() {
        let big = FileEntry {
            path: "Containerfile".into(),
            data: vec![b'#'; 64],
            mode: 0o644,
        };
        let blob = crate::build::pack_entries(&[big], MAX_FILESET_BYTES, MAX_FILESET_ENTRIES)
            .expect("packing");
        let err = read_within("./image/Containerfile", &blob, 8)
            .expect_err("a hostile artifact must not decide how much this machine allocates")
            .to_string();
        assert!(err.contains("expands past"), "{err}");
    }
}
