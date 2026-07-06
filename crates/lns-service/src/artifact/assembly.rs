use crate::artifact::spec::validate_mount_path;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Override {
    pub kind: String,
    pub name: String,
    pub mount_path: Option<String>,
}

pub fn apply_with(mut bundle: ResolvedBundle, overrides: &[Override]) -> Result<ResolvedBundle> {
    for over in overrides {
        match over.kind.as_str() {
            "FileSet" => {
                let Some(path) = over.mount_path.clone().filter(|p| !p.is_empty()) else {
                    bail!("--with FileSet override {} has no mount path", over.name);
                };
                validate_mount_path(&path)
                    .with_context(|| format!("--with FileSet override {}", over.name))?;
                bundle.filesets.push(ResolvedFileset {
                    name: over.name.clone(),
                    paths: vec![path],
                });
            }
            other => bail!(
                "--with override of kind {other} is unsupported; only FileSet overrides are allowed"
            ),
        }
    }
    Ok(bundle)
}

#[derive(Debug, Default, Clone)]
pub struct ResolvedBundle {
    pub base_image: String,
    pub base_paths: Vec<String>,
    pub filesets: Vec<ResolvedFileset>,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedFileset {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSource {
    BaseImage,
    Fileset(String),
}

#[derive(Debug, Clone)]
pub struct AssembledWorkload {
    pub base_image: String,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    ownership: BTreeMap<String, FileSource>,
}

impl AssembledWorkload {
    pub fn source_of(&self, path: &str) -> Option<&FileSource> {
        self.ownership.get(path)
    }
}

pub fn assemble(bundle: &ResolvedBundle) -> AssembledWorkload {
    let mut ownership = BTreeMap::new();
    for path in &bundle.base_paths {
        ownership.insert(path.clone(), FileSource::BaseImage);
    }
    for fileset in &bundle.filesets {
        for path in &fileset.paths {
            ownership.insert(path.clone(), FileSource::Fileset(fileset.name.clone()));
        }
    }
    AssembledWorkload {
        base_image: bundle.base_image.clone(),
        command: bundle.command.clone(),
        env: bundle.env.clone(),
        ownership,
    }
}
