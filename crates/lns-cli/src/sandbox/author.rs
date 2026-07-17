use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::SandboxCommand;

pub const LNS_YAML: &str = "lns.yaml";

const SCAFFOLD: &str = "apiVersion: lns.run/v1
kind: Sandbox
metadata:
  name: hermes
spec:
  image: docker.io/nousresearch/hermes-agent:latest
  command: gateway run
  workdir: /opt/data
  env:
    HERMES_DASHBOARD: \"1\"
  resources:
    cpu: 2
    memory: 4Gi
  policy:
    defaultVerdict: ask
    allowedRoutes: []
  integrations:
    - anthropic
  credentials: []
  volumes:
    - type: volume
      source: hermes-data
      target: /opt/data
  filesets:
    - path: ./skills
      mountPath: /opt/data/skills
  ports:
    - container: 8642
      host: 8642
    - container: 9119
      host: 9119
";

pub const SKILLS_SKILL_MD: &str = "skills/SKILL.md";

const SKILL_SCAFFOLD: &str = "---
name: hello-sandbox
description: Confirms mounted skills load by greeting from inside the sandbox.
---

When the user says hello, mention you are running inside a Lens Sandbox
microVM and that this skill was mounted from the project's skills/ directory.
";

/// A minimal filesystem seam so the author verbs are host-tested with an in-memory fake; `RealFs` in `real.rs` is the std::fs leaf.
pub trait Fs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn dir_entries(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
}

/// Derive a directory listing from a flat path-keyed map, so every in-memory fake Fs shares one implementation.
pub fn map_dir_entries<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    dir: &Path,
) -> io::Result<Vec<DirEntry>> {
    let mut seen: std::collections::BTreeMap<String, bool> = Default::default();
    for key in paths {
        let Ok(rest) = key.strip_prefix(dir) else {
            continue;
        };
        let mut components = rest.components();
        let Some(std::path::Component::Normal(first)) = components.next() else {
            continue;
        };
        let nested = components.next().is_some();
        let slot = seen
            .entry(first.to_string_lossy().into_owned())
            .or_default();
        *slot = *slot || nested;
    }
    if seen.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no such directory"));
    }
    Ok(seen
        .into_iter()
        .map(|(name, dir)| DirEntry { name, dir })
        .collect())
}

/// The author verbs run offline, against the working directory rather than the service.
pub fn is_offline(cmd: &SandboxCommand) -> bool {
    matches!(
        cmd,
        SandboxCommand::Init | SandboxCommand::Validate | SandboxCommand::Show
    )
}

fn yaml_path(cwd: &Path) -> PathBuf {
    cwd.join(LNS_YAML)
}

pub fn init<F: Fs, W: Write>(fs: &F, cwd: &Path, out: &mut W) -> Result<i32> {
    let path = yaml_path(cwd);
    if fs.exists(&path) {
        bail!("{LNS_YAML} already exists in this directory; not overwriting it");
    }
    fs.write(&path, SCAFFOLD)
        .with_context(|| format!("writing {}", path.display()))?;
    writeln!(
        out,
        "✓ created {LNS_YAML} — a Hermes agent sandbox, every field ready to edit"
    )?;
    let skill_path = cwd.join(SKILLS_SKILL_MD);
    if !fs.exists(&skill_path) {
        fs.write(&skill_path, SKILL_SCAFFOLD)
            .with_context(|| format!("writing {}", skill_path.display()))?;
        writeln!(
            out,
            "✓ created {SKILLS_SKILL_MD} — an example skill, mounted into the agent"
        )?;
    }
    writeln!(
        out,
        "\n  1. connect your model key: `lns integration connect anthropic`\n  2. boot it: `lns run`\n  3. share it: `lns push`, e.g. `lns push ghcr.io/acme/hermes:1.0.0`"
    )?;
    Ok(0)
}

pub fn load_definition_json<F: Fs>(fs: &F, cwd: &Path) -> Result<Vec<u8>> {
    let path = yaml_path(cwd);
    let yaml = fs
        .read_to_string(&path)
        .with_context(|| format!("reading {}; run `lns init` to scaffold one", path.display()))?;
    let value: serde_json::Value =
        serde_yaml::from_str(&yaml).with_context(|| format!("parsing {}", path.display()))?;
    serde_json::to_vec(&value).context("normalizing the definition to json")
}

pub fn validate<F: Fs, W: Write>(fs: &F, cwd: &Path, out: &mut W) -> Result<i32> {
    let json = load_definition_json(fs, cwd)?;
    let mut problems = match lns_artifact::validate::validate(&json) {
        Ok(()) => Vec::new(),
        Err(problems) => problems,
    };
    if problems.is_empty()
        && let Ok(def) = lns_artifact::sandbox::parse(&json)
    {
        problems.extend(super::fileset::path_fileset_problems(fs, cwd, &def));
    }
    if problems.is_empty() {
        writeln!(out, "{LNS_YAML} is valid.")?;
        return Ok(0);
    }
    writeln!(out, "{LNS_YAML} is not valid:")?;
    for problem in &problems {
        writeln!(out, "  - {problem}")?;
    }
    Ok(1)
}

pub fn show<F: Fs, W: Write>(fs: &F, cwd: &Path, out: &mut W) -> Result<i32> {
    let json = load_definition_json(fs, cwd)?;
    let def = lns_artifact::sandbox::parse(&json)
        .map_err(|e| anyhow::anyhow!("{LNS_YAML} is not a valid sandbox: {e:#}"))?;
    render_effective(&def, out)?;
    Ok(0)
}

fn render_effective<W: Write>(def: &lns_artifact::sandbox::Definition, out: &mut W) -> Result<()> {
    let verdict = serde_json::to_string(&def.spec.policy.default_verdict).unwrap_or_default();
    writeln!(out, "Sandbox: {}", def.metadata.name)?;
    writeln!(out, "  image:        {}", def.spec.image)?;
    if let Some(command) = &def.spec.command {
        writeln!(out, "  command:      {command}")?;
    }
    if let Some(workdir) = &def.spec.workdir {
        writeln!(out, "  workdir:      {workdir}")?;
    }
    for volume in &def.spec.volumes {
        let kind = if volume.is_bind() { "bind" } else { "volume" };
        let mode = if volume.read_only() {
            "read-only"
        } else {
            "read-write"
        };
        writeln!(
            out,
            "  mount:        {kind} {} -> {} ({mode})",
            volume.source(),
            volume.target
        )?;
    }
    for fileset in &def.spec.filesets {
        let source = fileset
            .path
            .as_deref()
            .or(fileset.reference.as_deref())
            .unwrap_or_default();
        writeln!(out, "  fileset:      {source} -> {}", fileset.mount_path)?;
    }
    writeln!(
        out,
        "  policy:       defaultVerdict={} ({} route(s))",
        verdict.trim_matches('"'),
        def.spec.policy.allowed_routes.len()
    )?;
    if !def.spec.integrations.is_empty() {
        writeln!(out, "  integrations: {}", def.spec.integrations.join(", "))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::sandbox::test_support::MapFs;

    fn fake(path: &str, contents: &str) -> MapFs {
        MapFs::with(&[(path, contents)])
    }

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn valid_yaml() -> &'static str {
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  integrations: [some-provider]\n"
    }

    #[test]
    fn is_offline_matches_the_author_verbs_only() {
        assert!(is_offline(&SandboxCommand::Init));
        assert!(is_offline(&SandboxCommand::Validate));
        assert!(is_offline(&SandboxCommand::Show));
        assert!(!is_offline(&SandboxCommand::Ps));
        assert!(!is_offline(&SandboxCommand::Ls));
    }

    #[test]
    fn init_scaffolds_an_agent_example_with_a_skill() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let code = init(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        let written = fs.read_to_string(&yaml_path(cwd())).unwrap();
        assert!(written.contains("kind: Sandbox"));
        assert!(written.contains("apiVersion: lns.run/v1"));
        let skill = fs.read_to_string(&cwd().join(SKILLS_SKILL_MD)).unwrap();
        assert!(skill.contains("name: hello-sandbox"), "got: {skill}");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("✓ created lns.yaml"), "got: {text}");
        assert!(text.contains("✓ created skills/SKILL.md"), "got: {text}");
        assert!(text.contains("`lns run`"), "got: {text}");
    }

    #[test]
    fn init_keeps_an_existing_skill_file() {
        let fs = fake("/work/skills/SKILL.md", "mine");
        let mut out = Vec::new();
        let code = init(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            fs.read_to_string(&cwd().join(SKILLS_SKILL_MD)).unwrap(),
            "mine"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("✓ created skills/SKILL.md"), "got: {text}");
    }

    #[test]
    fn init_refuses_to_clobber_an_existing_definition() {
        let fs = fake("/work/lns.yaml", "keep me");
        let mut out = Vec::new();
        let err = init(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
        assert_eq!(fs.read_to_string(&yaml_path(cwd())).unwrap(), "keep me");
    }

    #[test]
    fn init_surfaces_a_write_failure() {
        let fs = MapFs {
            fail_write: true,
            ..Default::default()
        };
        let mut out = Vec::new();
        let err = init(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
    }

    #[test]
    fn validate_passes_a_well_formed_definition() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("is valid"));
    }

    #[test]
    fn validate_surfaces_a_missing_file() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("lns init"));
    }

    #[test]
    fn validate_surfaces_malformed_yaml() {
        let fs = fake("/work/lns.yaml", "spec: [unterminated");
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("parsing"));
    }

    #[test]
    fn show_renders_the_effective_definition() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = show(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("image:") && text.contains("ghcr.io/team/base:1"),
            "got: {text}"
        );
        assert!(
            text.contains("policy:") && text.contains("defaultVerdict=ask"),
            "got: {text}"
        );
        assert!(text.contains("integrations: some-provider"), "got: {text}");
    }

    #[test]
    fn show_renders_a_command_when_present() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  command: agent --serve\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        show(&fs, cwd(), &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("command:      agent --serve")
        );
    }

    #[test]
    fn validate_reports_a_broken_path_fileset_with_its_path() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
        );
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("fileset ./skills"), "got: {text}");
    }

    #[test]
    fn map_dir_entries_treats_a_file_path_as_no_directory() {
        let files: HashMap<PathBuf, String> =
            [(PathBuf::from("/work/skills"), String::new())].into();
        let err = map_dir_entries(files.keys(), Path::new("/work/skills")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn show_renders_path_and_ref_filesets() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n    - ref: registry.example.test/team/settings@sha256:abc\n      mountPath: /root/.agent/settings\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        show(&fs, cwd(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("fileset:      ./skills -> /root/.agent/skills"),
            "got: {text}"
        );
        assert!(
            text.contains(
                "fileset:      registry.example.test/team/settings@sha256:abc -> /root/.agent/settings"
            ),
            "got: {text}"
        );
    }

    #[test]
    fn show_refuses_an_invalid_definition() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let mut out = Vec::new();
        let err = show(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("not a valid sandbox"));
    }
}
