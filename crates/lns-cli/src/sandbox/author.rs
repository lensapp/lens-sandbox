use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::SandboxCommand;

pub const LNS_YAML: &str = "lns.yaml";

const SCAFFOLD: &str = "apiVersion: lns.run/v1
kind: Sandbox
metadata:
  name: sandbox
spec:
  image: docker.io/library/alpine:3.20
  command: sh
  workdir: /workspace
  env: {}
  resources:
    cpu: 1
    memory: 512Mi
  policy:
    defaultVerdict: ask
    allowedRoutes: []
  connectors: []
  credentials: []
  volumes:
    - type: bind
      source: .
      target: /workspace
  filesets: []
  ports: []
";

/// A minimal filesystem seam so the author verbs are host-tested with an in-memory fake; `RealFs` in `real.rs` is the std::fs leaf.
pub trait Fs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn read_limited(&self, path: &Path, max_bytes: u64) -> io::Result<Vec<u8>>;
    fn dir_entries(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub mode: u32,
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
        .map(|(name, dir)| DirEntry {
            name,
            dir,
            mode: if dir { 0o755 } else { 0o644 },
        })
        .collect())
}

/// The author verbs run offline, against the working directory rather than the service; inspect joins them when its target is a local definition (or omitted).
pub fn is_offline(cmd: &SandboxCommand) -> bool {
    match cmd {
        SandboxCommand::Init | SandboxCommand::Validate(_) => true,
        SandboxCommand::Inspect(args) => {
            args.file.is_some() || is_local_inspect(args.run.as_deref())
        }
        _ => false,
    }
}

pub fn is_local_inspect(target: Option<&str>) -> bool {
    match target {
        None => true,
        Some(target) => crate::run::target::is_definition_path(target),
    }
}

fn yaml_path(cwd: &Path) -> PathBuf {
    cwd.join(LNS_YAML)
}

/// The `--file` selector names the definition to operate on; the default stays `./lns.yaml`.
pub fn selected_definition_path(file: Option<&Path>, cwd: &Path) -> PathBuf {
    match file {
        Some(file) => crate::run::target::normalize(&cwd.join(file)),
        None => yaml_path(cwd),
    }
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
        "✓ created {LNS_YAML} — your sandbox definition, every field ready to edit\n\n  1. set spec.image (scaffolded to alpine:3.20)\n  2. boot it with `lns run`\n  3. share it with `lns push`, e.g. `lns push ghcr.io/acme/my-sandbox:1.0.0`"
    )?;
    Ok(0)
}

pub fn load_definition_json_at<F: Fs>(fs: &F, path: &Path) -> Result<Vec<u8>> {
    let hint = if path.file_name() == Some(LNS_YAML.as_ref()) {
        "; run `lns init` to scaffold one"
    } else {
        ""
    };
    let yaml = fs
        .read_to_string(path)
        .with_context(|| format!("reading {}{hint}", path.display()))?;
    let value: serde_json::Value =
        serde_yaml::from_str(&yaml).with_context(|| format!("parsing {}", path.display()))?;
    serde_json::to_vec(&value).context("normalizing the definition to json")
}

pub fn validate<F: Fs, W: Write>(
    fs: &F,
    cwd: &Path,
    file: Option<&Path>,
    out: &mut W,
) -> Result<i32> {
    let path = selected_definition_path(file, cwd);
    let project_dir = path.parent().unwrap_or(cwd);
    let name = file
        .map(|f| f.display().to_string())
        .unwrap_or_else(|| LNS_YAML.to_string());
    let json = load_definition_json_at(fs, &path)?;
    let mut problems = match lns_artifact::validate::validate(&json) {
        Ok(()) => Vec::new(),
        Err(problems) => problems,
    };
    if problems.is_empty()
        && let Ok(def) = lns_artifact::sandbox::parse(&json)
    {
        problems.extend(super::fileset::path_fileset_problems(fs, project_dir, &def));
    }
    if problems.is_empty() {
        writeln!(out, "{name} is valid.")?;
        return Ok(0);
    }
    writeln!(out, "{name} is not valid:")?;
    for problem in &problems {
        writeln!(out, "  - {problem}")?;
    }
    Ok(1)
}

pub fn inspect_local<F: Fs, W: Write>(
    fs: &F,
    cwd: &Path,
    target: Option<&str>,
    file: Option<&Path>,
    out: &mut W,
) -> Result<i32> {
    let path = match (target, file) {
        (Some(_), Some(_)) => bail!("pass an inspect target or --file, not both"),
        (Some(target), None) => crate::run::target::definition_file(target, cwd),
        (None, file) => selected_definition_path(file, cwd),
    };
    let json = load_definition_json_at(fs, &path)?;
    let def = lns_artifact::sandbox::parse(&json)
        .map_err(|e| anyhow::anyhow!("{} is not a valid sandbox: {e:#}", path.display()))?;
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
    for (key, value) in &def.spec.env {
        writeln!(out, "  env:          {key}={value}")?;
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
        let source = crate::run::summary::fileset_source_display(fileset);
        let owner = crate::run::summary::fileset_owner_display(fileset.owner);
        writeln!(
            out,
            "  fileset:      {source} -> {} (owner: {owner})",
            fileset.mount_path
        )?;
    }
    writeln!(
        out,
        "  policy:       defaultVerdict={} ({} route(s))",
        verdict.trim_matches('"'),
        def.spec.policy.allowed_routes.len()
    )?;
    if !def.spec.connectors.is_empty() {
        writeln!(out, "  connectors: {}", def.spec.connectors.join(", "))?;
    }
    for credential in &def.spec.credentials {
        let required = if credential.required {
            " (required)"
        } else {
            ""
        };
        let name = &credential.name;
        let env = &credential.env;
        writeln!(out, "  credential: {name} -> {env}{required}")?;
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
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n  connectors: [some-provider]\n"
    }

    fn inspect_cmd(target: Option<&str>) -> SandboxCommand {
        SandboxCommand::Inspect(crate::sandbox::SandboxInspectArgs {
            run: target.map(str::to_string),
            file: None,
        })
    }

    #[test]
    fn is_offline_matches_the_author_verbs_and_local_inspects_only() {
        assert!(is_offline(&SandboxCommand::Init));
        assert!(is_offline(&SandboxCommand::Validate(
            crate::sandbox::SandboxValidateArgs { file: None }
        )));
        assert!(is_offline(&inspect_cmd(None)));
        assert!(is_offline(&inspect_cmd(Some("."))));
        assert!(is_offline(&inspect_cmd(Some("lns.yaml"))));
        assert!(is_offline(&inspect_cmd(Some("../other"))));
        assert!(!is_offline(&inspect_cmd(Some("ghcr.io/team/hermes:1"))));
        assert!(!is_offline(&inspect_cmd(Some("brave_narwhal"))));
        assert!(!is_offline(&SandboxCommand::Ps));
        assert!(!is_offline(&SandboxCommand::Ls));
    }

    #[test]
    fn init_scaffolds_a_default_definition() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let code = init(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        let written = fs.read_to_string(&yaml_path(cwd())).unwrap();
        assert!(written.contains("kind: Sandbox"));
        assert!(written.contains("apiVersion: lns.run/v1"));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("✓ created lns.yaml"), "got: {text}");
        assert!(text.contains("`lns run`"), "got: {text}");
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
        let code = validate(&fs, cwd(), None, &mut out).unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("is valid"));
    }

    #[test]
    fn validate_file_selector_roots_filesets_at_the_files_directory_and_names_it() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: dev\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n";
        let fs = MapFs::with(&[
            ("/other/lns.dev.yaml", yaml),
            ("/other/skills/prompts.md", "p"),
        ]);
        let mut out = Vec::new();
        let code = validate(
            &fs,
            cwd(),
            Some(Path::new("../other/lns.dev.yaml")),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("../other/lns.dev.yaml is valid."),
            "got: {text}"
        );
    }

    #[test]
    fn validate_file_selector_reports_problems_under_the_selected_name() {
        let fs = fake(
            "/work/lns.dev.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: dev\nspec: {}\n",
        );
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), Some(Path::new("lns.dev.yaml")), &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("lns.dev.yaml is not valid:"), "got: {text}");
    }

    #[test]
    fn validate_surfaces_a_missing_file() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("lns init"));
    }

    #[test]
    fn a_missing_variant_file_does_not_hint_lns_init() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), Some(Path::new("lns.dev.yaml")), &mut out).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("lns.dev.yaml"), "got: {text}");
        assert!(
            !text.contains("lns init"),
            "`lns init` cannot scaffold a variant: {text}"
        );
    }

    #[test]
    fn validate_surfaces_malformed_yaml() {
        let fs = fake("/work/lns.yaml", "spec: [unterminated");
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("parsing"));
    }

    #[test]
    fn inspect_local_renders_the_effective_definition() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), None, None, &mut out).unwrap();
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
        assert!(text.contains("connectors: some-provider"), "got: {text}");
    }

    #[test]
    fn inspect_local_renders_a_command_when_present() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  command: agent --serve\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("command:      agent --serve")
        );
    }

    #[test]
    fn inspect_local_renders_env_sorted_by_key() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  env:\n    SHELL: /bin/sh\n    FOO: bar\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("env:          FOO=bar"), "got: {text}");
        assert!(text.contains("env:          SHELL=/bin/sh"), "got: {text}");
        assert!(
            text.find("FOO=bar").unwrap() < text.find("SHELL=/bin/sh").unwrap(),
            "env should render sorted by key: {text}"
        );
    }

    #[test]
    fn validate_reports_a_broken_path_fileset_with_its_path() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n",
        );
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), None, &mut out).unwrap();
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
    fn inspect_local_renders_path_and_ref_filesets() {
        let yaml = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      mountPath: /root/.agent/skills\n    - ref: registry.example.test/team/settings@sha256:abc\n      mountPath: /root/.agent/settings\n  credentials:\n    - name: some-provider\n      env: SOME_TOKEN\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), Some("."), None, &mut out).unwrap();
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
        assert!(
            text.contains("credential: some-provider -> SOME_TOKEN"),
            "got: {text}"
        );
    }

    #[test]
    fn inspect_local_renders_another_directorys_definition_by_path() {
        let fs = fake("/other/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), Some("../other"), None, &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("ghcr.io/team/base:1"), "got: {text}");
    }

    #[test]
    fn inspect_file_selector_renders_the_named_definition() {
        let variant = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: dev\nspec:\n  image: ghcr.io/team/dev:1\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            ("/work/lns.dev.yaml", variant),
        ]);
        let mut out = Vec::new();
        let code =
            inspect_local(&fs, cwd(), None, Some(Path::new("lns.dev.yaml")), &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("ghcr.io/team/dev:1"), "got: {text}");
    }

    #[test]
    fn inspect_refuses_a_target_and_file_selector_together() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let err = inspect_local(
            &fs,
            cwd(),
            Some("."),
            Some(Path::new("lns.dev.yaml")),
            &mut out,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not both"), "got: {err:#}");
    }

    #[test]
    fn a_file_selector_inspect_is_offline() {
        assert!(is_offline(&SandboxCommand::Inspect(
            crate::sandbox::SandboxInspectArgs {
                run: None,
                file: Some(std::path::PathBuf::from("lns.dev.yaml")),
            }
        )));
    }

    #[test]
    fn inspect_local_refuses_an_invalid_definition() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let mut out = Vec::new();
        let err = inspect_local(&fs, cwd(), None, None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("not a valid sandbox"));
    }
}
