use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use lns_artifact::image::{CONTAINERFILE_NAMES, ImageSource, source};

use super::author::Fs;

/// What a path-form `spec.image` discloses before anything is built: the Containerfile lns would build, its text, and the files the context would carry (§3.1.1).
pub struct BuiltImage {
    pub containerfile: String,
    pub text: String,
    pub context: Vec<ContextFile>,
}

/// One file of the build context, as an approver reads it: the path inside the context, its size, and whether a build sends it — a symlink is listed and not sent (§3.1.1).
pub struct ContextFile {
    pub path: String,
    pub bytes: u64,
    pub sent: bool,
}

impl ContextFile {
    /// What one context line discloses: a size for a file a build sends, and the reason for one it does not.
    pub fn disclosure(&self) -> String {
        if self.sent {
            crate::output::format_bytes(self.bytes)
        } else {
            "symlink, not sent".to_string()
        }
    }
}

impl BuiltImage {
    /// The image line of a summary: which file builds this sandbox, and how much there is to read.
    pub fn summary(&self) -> String {
        format!(
            "built from {} ({} lines, context {} files)",
            self.containerfile,
            self.text.lines().count(),
            self.context.iter().filter(|file| file.sent).count()
        )
    }
}

/// The Containerfile a path-form `spec.image` names, spelled as the author wrote it and paired with the absolute path this machine reads it at: a directory holds one under either name — `Containerfile` first, as Podman does — and a path naming a file is the Containerfile whatever it is called (§3.1.1).
fn resolve<F: Fs + ?Sized>(fs: &F, project_dir: &Path, path: &str) -> Result<(String, PathBuf)> {
    let named = lns_artifact::sandbox::fold_path(&project_dir.join(path));
    let stem = path.trim_end_matches('/');
    if fs.is_dir(&named) {
        return CONTAINERFILE_NAMES
            .iter()
            .find(|name| fs.exists(&named.join(name)))
            .map(|name| (format!("{stem}/{name}"), named.join(name)))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "spec.image {path:?} names a directory holding no Containerfile or Dockerfile"
                )
            });
    }
    if fs.exists(&named) {
        return Ok((stem.to_string(), named));
    }
    bail!("spec.image {path:?} names no file or directory beside the document")
}

/// The offline validate/inspect guard: a path-form image must name a Containerfile this machine can read and lns can build, so a typo and an unbuildable instruction are both found before a push.
pub fn image_problems<F: Fs + ?Sized>(fs: &F, project_dir: &Path, image: &str) -> Vec<String> {
    let (containerfile, at, text) = match containerfile_text(fs, project_dir, image) {
        Ok(None) => return Vec::new(),
        Ok(Some(read)) => read,
        Err(e) => return vec![format!("{e:#}")],
    };
    let refusals = match lns_artifact::containerfile::parse(&text) {
        Ok(built) => archives_an_add_would_unpack(fs, at.parent().unwrap_or(project_dir), &built),
        Err(refusals) => refusals,
    };
    refusals
        .iter()
        .map(|refusal| format!("{containerfile} {refusal}"))
        .collect()
}

/// Docker decides what `ADD` unpacks by content, so a source that is an archive under any name is refused where the file can still be read — the parse-time suffix check is what is left when it cannot.
fn archives_an_add_would_unpack<F: Fs + ?Sized>(
    fs: &F,
    context: &Path,
    built: &lns_artifact::containerfile::Containerfile,
) -> Vec<String> {
    use lns_artifact::containerfile::{
        ARCHIVE_SNIFF_BYTES, InstructionKind, looks_like_an_archive, unpacks_an_archive,
    };
    let mut refusals = Vec::new();
    for instruction in &built.instructions {
        let InstructionKind::Add(transfer) = &instruction.kind else {
            continue;
        };
        for source in &transfer.sources {
            let at = lns_artifact::sandbox::fold_path(&context.join(source));
            let Ok(head) = fs.read_limited(&at, ARCHIVE_SNIFF_BYTES) else {
                continue;
            };
            if looks_like_an_archive(&head) {
                refusals.push(unpacks_an_archive(instruction.line, source));
            }
        }
    }
    refusals
}

/// The Containerfile a path-form `spec.image` names, as written and as read; the subset gate needs no more than this, and the context walk is `inspect`'s alone.
fn containerfile_text<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    image: &str,
) -> Result<Option<(String, PathBuf, String)>> {
    let ImageSource::Containerfile(path) = source(image) else {
        return Ok(None);
    };
    let (containerfile, at) = resolve(fs, project_dir, path)?;
    let text = fs
        .read_to_string(&at)
        .with_context(|| format!("reading {containerfile}"))?;
    Ok(Some((containerfile, at, text)))
}

/// What a render discloses about the image, and `None` for one that is pulled rather than built.
pub fn built_from<F: Fs + ?Sized>(
    fs: &F,
    project_dir: &Path,
    image: &str,
) -> Result<Option<BuiltImage>> {
    let Some((containerfile, at, text)) = containerfile_text(fs, project_dir, image)? else {
        return Ok(None);
    };
    let context = context_files(fs, at.parent().unwrap_or(project_dir))?;
    Ok(Some(BuiltImage {
        containerfile,
        text,
        context,
    }))
}

/// Every file a build would send, because the context is what the artifact ships and an approver reads it before it is packed.
fn context_files<F: Fs + ?Sized>(fs: &F, root: &Path) -> Result<Vec<ContextFile>> {
    let mut files = Vec::new();
    collect_context(fs, root, Path::new(""), &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_context<F: Fs + ?Sized>(
    fs: &F,
    dir: &Path,
    rel: &Path,
    out: &mut Vec<ContextFile>,
) -> Result<()> {
    let listed = fs
        .dir_entries(dir)
        .with_context(|| format!("reading the build context {}", dir.display()))?;
    for entry in listed {
        let entry_rel = rel.join(&entry.name);
        if entry.symlink {
            out.push(ContextFile {
                path: entry_rel.display().to_string(),
                bytes: 0,
                sent: false,
            });
            continue;
        }
        if entry.dir {
            collect_context(fs, &dir.join(&entry.name), &entry_rel, out)?;
            continue;
        }
        let bytes = fs
            .size(&dir.join(&entry.name))
            .with_context(|| format!("sizing the build context file {}", entry_rel.display()))?;
        out.push(ContextFile {
            path: entry_rel.display().to_string(),
            bytes,
            sent: true,
        });
    }
    Ok(())
}

/// A path-form `spec.image` publishes as the digest of an image lns built ([§6](docs/sandbox-spec.md)), and lns has no builder yet, so the verbs that would need one refuse rather than hand a path to a registry.
pub fn refuse_an_unbuilt_image(image: &str) -> Result<()> {
    if let ImageSource::Containerfile(path) = source(image) {
        bail!(
            "spec.image {path:?} names a Containerfile, and building one is not yet supported; publish the image yourself and name it by reference"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::test_support::MapFs;

    fn context_with_a_symlink() -> MapFs {
        let mut fs = MapFs::with(&[
            ("/p/image/Containerfile", "FROM alpine\nCOPY ./app /srv\n"),
            ("/p/image/app/main.js", "console.log(1)\n"),
            (
                "/p/image/node_modules/typescript/bin/tsc",
                "#!/usr/bin/env node\n",
            ),
        ]);
        fs.symlinks
            .insert(PathBuf::from("/p/image/node_modules/.bin/tsc"));
        fs
    }

    #[test]
    fn a_context_file_is_sized_rather_than_read_so_a_big_one_reads_true_and_costs_nothing() {
        let mut fs = MapFs::with(&[
            ("/p/image/Containerfile", "FROM alpine\n"),
            ("/p/image/big.bin", "a stand-in for half a gigabyte"),
        ]);
        fs.sizes
            .insert(PathBuf::from("/p/image/big.bin"), 500 * 1024 * 1024);
        let built = built_from(&fs, Path::new("/p"), "./image")
            .expect("reading")
            .expect("a path-form image is built");
        assert_eq!(
            built.context[1].disclosure(),
            "500.0 MiB",
            "the size is the file's, not what a capped read returned"
        );
        let touched = fs.touched.borrow().join(", ");
        assert_eq!(
            touched.as_str(),
            "list /p/image, size /p/image/Containerfile, size /p/image/big.bin"
        );
    }

    #[test]
    fn validate_does_not_walk_the_context_because_it_reads_only_the_containerfile() {
        let fs = MapFs::with(&[
            ("/p/image/Containerfile", "FROM alpine\n"),
            ("/p/image/app/main.js", "console.log(1)\n"),
        ]);
        assert_eq!(
            image_problems(&fs, Path::new("/p"), "./image"),
            Vec::<String>::new()
        );
        let touched = fs.touched.borrow().join(", ");
        assert_eq!(
            touched.as_str(),
            "",
            "the subset gate lists nothing and sizes nothing"
        );
    }

    #[test]
    fn an_add_of_an_archive_with_no_archive_extension_is_refused_by_what_it_holds() {
        let mut tar = vec![b'0'; 512];
        tar[257..262].copy_from_slice(b"ustar");
        let fs = MapFs::with(&[
            (
                "/p/image/Containerfile",
                "FROM alpine\nADD ./toolchain /opt/toolchain\n",
            ),
            (
                "/p/image/toolchain",
                &String::from_utf8(tar).expect("the fake holds text"),
            ),
        ]);
        let problems = image_problems(&fs, Path::new("/p"), "./image");
        assert_eq!(problems.len(), 1, "got: {problems:?}");
        let refusal = problems[0].clone();
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("./toolchain"), "{refusal}");
        assert!(refusal.contains("RUN tar"), "{refusal}");
    }

    #[test]
    fn an_add_of_an_ordinary_file_is_left_alone() {
        let fs = MapFs::with(&[
            ("/p/image/Containerfile", "FROM alpine\nADD ./e.sh /e.sh\n"),
            ("/p/image/e.sh", "#!/bin/sh\nexec node .\n"),
        ]);
        assert_eq!(
            image_problems(&fs, Path::new("/p"), "./image"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_add_source_the_context_does_not_hold_is_left_to_the_build_to_fail_on() {
        let fs = MapFs::with(&[(
            "/p/image/Containerfile",
            "FROM alpine\nADD ./absent /opt/absent\n",
        )]);
        assert_eq!(
            image_problems(&fs, Path::new("/p"), "./image"),
            Vec::<String>::new(),
            "a source that cannot be read cannot be sniffed, and a missing file is not this check's refusal"
        );
    }

    #[test]
    fn a_symlink_in_the_context_is_disclosed_as_not_sent_rather_than_failing_the_walk() {
        let built = built_from(&context_with_a_symlink(), Path::new("/p"), "./image")
            .expect("a context an npm install wrote still reads")
            .expect("a path-form image is built");
        let mut listed = Vec::new();
        for file in &built.context {
            listed.push(format!("{} sent={}", file.path, file.sent));
        }
        let listed = listed.join(", ");
        assert_eq!(
            listed.as_str(),
            "Containerfile sent=true, app/main.js sent=true, node_modules/.bin/tsc sent=false, node_modules/typescript/bin/tsc sent=true",
            "the symlink is listed, and named as one a build does not send"
        );
    }

    #[test]
    fn the_summary_counts_the_files_a_build_would_send() {
        let built = built_from(&context_with_a_symlink(), Path::new("/p"), "./image")
            .expect("reading")
            .expect("a path-form image is built");
        let summary = built.summary();
        assert!(
            summary.contains("context 3 files"),
            "the skipped symlink is not one of them: {summary}"
        );
    }

    #[test]
    fn a_symlink_in_the_context_does_not_stop_validate_reading_the_containerfile() {
        let problems = image_problems(&context_with_a_symlink(), Path::new("/p"), "./image");
        assert!(problems.is_empty(), "got: {problems:?}");
    }
}
