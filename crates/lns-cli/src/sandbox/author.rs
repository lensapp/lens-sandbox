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
  # The base OCI image this sandbox runs; pin it by digest before you publish.
  image: docker.io/library/alpine:3.20
  # command: sh
  # env:
  #   MODE: production
  # policy:
  #   defaultVerdict: ask
  # integrations: []
";

/// A minimal filesystem seam so the author verbs are host-tested with an in-memory fake; `RealFs` in `real.rs` is the std::fs leaf.
pub trait Fs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
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
    writeln!(out, "Created {LNS_YAML} — edit spec.image, then `lns run`.")?;
    Ok(0)
}

pub(crate) fn load_definition_json<F: Fs>(fs: &F, cwd: &Path) -> Result<Vec<u8>> {
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
    match lns_artifact::validate::validate(&json) {
        Ok(()) => {
            writeln!(out, "{LNS_YAML} is valid.")?;
            Ok(0)
        }
        Err(problems) => {
            writeln!(out, "{LNS_YAML} is not valid:")?;
            for problem in &problems {
                writeln!(out, "  - {problem}")?;
            }
            Ok(1)
        }
    }
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
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeFs {
        files: RefCell<HashMap<PathBuf, String>>,
    }

    impl FakeFs {
        fn with(path: &str, contents: &str) -> Self {
            let fs = Self::default();
            fs.files
                .borrow_mut()
                .insert(PathBuf::from(path), contents.to_string());
            fs
        }
    }

    impl Fs for FakeFs {
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            self.files
                .borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
        fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
            self.files
                .borrow_mut()
                .insert(path.to_path_buf(), contents.to_string());
            Ok(())
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.borrow().contains_key(path)
        }
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
        assert!(!is_offline(&SandboxCommand::Prune));
        assert!(!is_offline(&SandboxCommand::Ls));
    }

    #[test]
    fn init_scaffolds_a_default_definition() {
        let fs = FakeFs::default();
        let mut out = Vec::new();
        let code = init(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        let written = fs.read_to_string(&yaml_path(cwd())).unwrap();
        assert!(written.contains("kind: Sandbox"));
        assert!(written.contains("apiVersion: lns.run/v1"));
        assert!(String::from_utf8(out).unwrap().contains("Created lns.yaml"));
    }

    #[test]
    fn init_refuses_to_clobber_an_existing_definition() {
        let fs = FakeFs::with("/work/lns.yaml", "keep me");
        let mut out = Vec::new();
        let err = init(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
        assert_eq!(fs.read_to_string(&yaml_path(cwd())).unwrap(), "keep me");
    }

    #[test]
    fn init_surfaces_a_write_failure() {
        struct FailWrite;
        impl Fs for FailWrite {
            fn read_to_string(&self, _: &Path) -> io::Result<String> {
                unreachable!()
            }
            fn write(&self, _: &Path, _: &str) -> io::Result<()> {
                Err(io::Error::other("disk full"))
            }
            fn exists(&self, _: &Path) -> bool {
                false
            }
        }
        let mut out = Vec::new();
        let err = init(&FailWrite, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
    }

    #[test]
    fn validate_passes_a_well_formed_definition() {
        let fs = FakeFs::with("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("is valid"));
    }

    #[test]
    fn validate_reports_a_real_secret_and_exits_nonzero() {
        let yaml = format!(
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: x:1\n  env:\n    GH_TOKEN: ghp_{}\n",
            "a".repeat(36)
        );
        let fs = FakeFs::with("/work/lns.yaml", &yaml);
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("not valid") && text.contains("GitHub token"),
            "got: {text}"
        );
    }

    #[test]
    fn validate_surfaces_a_missing_file() {
        let fs = FakeFs::default();
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("lns init"));
    }

    #[test]
    fn validate_surfaces_malformed_yaml() {
        let fs = FakeFs::with("/work/lns.yaml", "spec: [unterminated");
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("parsing"));
    }

    #[test]
    fn show_renders_the_effective_definition() {
        let fs = FakeFs::with("/work/lns.yaml", valid_yaml());
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
        let fs = FakeFs::with("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        show(&fs, cwd(), &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("command:      agent --serve")
        );
    }

    #[test]
    fn show_refuses_an_invalid_definition() {
        let fs = FakeFs::with(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let mut out = Vec::new();
        let err = show(&fs, cwd(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("not a valid sandbox"));
    }
}
