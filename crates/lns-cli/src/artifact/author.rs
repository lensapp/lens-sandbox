use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{ArtifactCommand, DocumentKind};

pub const LNS_YAML: &str = "lns.yaml";

const MIXIN_SCAFFOLD: &str = "apiVersion: lns.run/v1
kind: mixin
name: mixin
spec:
  egress:
    http: []
    tcp: []
  filesets: []
  tools: []
";

const SANDBOX_SCAFFOLD: &str = "apiVersion: lns.run/v1
kind: sandbox
name: sandbox
spec:
  image: alpine:3.20
  command: sh
  workdir: /workspace
  env: {}
  resources:
    cpu: 1
    memory: 512Mi
  egress:
    http: []
    tcp: []
  volumes:
    - type: bind
      source: .
      target: /workspace
  filesets: []
  ports: []
  tools: []
";

/// A minimal filesystem seam so the author verbs are host-tested with an in-memory fake; `RealFs` in `real.rs` is the std::fs leaf.
pub trait Fs: lns_artifact::walk::SnapshotFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, contents: &str) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn is_symlink(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

/// The author verbs run offline, against the working directory rather than the service; inspect joins them when its target is a local definition (or omitted).
pub fn is_offline(cmd: &ArtifactCommand) -> bool {
    match cmd {
        ArtifactCommand::Init(_) | ArtifactCommand::Validate(_) => true,
        ArtifactCommand::Inspect(args) => {
            args.file.is_some() || is_local_inspect(args.reference.as_deref())
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
        Some(file) => lns_artifact::sandbox::fold_path(&cwd.join(file)),
        None => yaml_path(cwd),
    }
}

pub fn init<F: Fs, E: Write>(
    fs: &F,
    cwd: &Path,
    kind: DocumentKind,
    file: Option<&Path>,
    err: &mut E,
) -> Result<i32> {
    let path = selected_definition_path(file, cwd);
    let name = file.map_or_else(|| LNS_YAML.to_string(), |f| f.display().to_string());
    if fs.exists(&path) {
        bail!("{name} already exists in this directory; not overwriting it");
    }
    let (scaffold, next_steps) = match kind {
        DocumentKind::Sandbox => (
            SANDBOX_SCAFFOLD,
            "  1. set spec.image (scaffolded to alpine:3.20)\n  2. boot it with `lns run`\n  3. share it with `lns push`, e.g. `lns push ghcr.io/acme/my-sandbox:1.0.0`",
        ),
        DocumentKind::Mixin => (
            MIXIN_SCAFFOLD,
            "  1. name the capability it layers on — tools, filesets, egress\n  2. try it with `lns run --mixin .`\n  3. share it with `lns push`, e.g. `lns push ghcr.io/acme/my-mixin:1.0.0`",
        ),
    };
    fs.write(&path, scaffold)
        .with_context(|| format!("writing {}", path.display()))?;
    let noun = match kind {
        DocumentKind::Sandbox => "sandbox definition",
        DocumentKind::Mixin => "mixin",
    };
    writeln!(
        err,
        "✓ created {name} — your {noun}, every field ready to edit\n\n{next_steps}"
    )?;
    Ok(0)
}

pub fn load_definition_json_at<F: Fs + ?Sized>(fs: &F, path: &Path) -> Result<Vec<u8>> {
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
    kind: Option<DocumentKind>,
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
    problems.extend(wrong_kind_problem(&json, kind));
    if problems.is_empty()
        && let Ok(def) = lns_artifact::sandbox::parse_document(&json)
        && let kind =
            lns_artifact::spec::read_kind(&json).unwrap_or(lns_artifact::spec::Kind::Sandbox)
    {
        let paths = def.path_filesets();
        let methods: &[lns_artifact::connector::Method] = match &def {
            lns_artifact::sandbox::Document::Connector(connector) => &connector.spec.methods,
            _ => &[],
        };
        problems.extend(super::fileset::path_fileset_problems(
            fs,
            project_dir,
            &paths,
            methods,
            kind,
        ));
        problems.extend(super::fileset::directory_mixin_problems(
            fs,
            project_dir,
            def.mixins(),
        ));
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

/// `--kind` is the caller saying which document they think this is, so a file of another kind is a problem alongside whatever else validation found rather than instead of it.
fn wrong_kind_problem(json: &[u8], wanted: Option<DocumentKind>) -> Option<String> {
    let wanted = wanted?.as_artifact_kind();
    let found = lns_artifact::spec::read_kind(json).ok()?;
    (found != wanted).then(|| {
        format!(
            "kind: expected a {}, but this is a {}",
            wanted.as_str(),
            found.as_str()
        )
    })
}

pub fn inspect_local<F: Fs, W: Write>(
    fs: &F,
    cwd: &Path,
    target: Option<&str>,
    file: Option<&Path>,
    mixins: &[String],
    out: &mut W,
) -> Result<i32> {
    let path = match (target, file) {
        (Some(_), Some(_)) => bail!("pass an inspect target or --file, not both"),
        (Some(target), None) => crate::run::target::definition_file(target, cwd),
        (None, file) => selected_definition_path(file, cwd),
    };
    let json = load_definition_json_at(fs, &path)?;
    let kind = lns_artifact::spec::read_kind(&json)
        .map_err(|e| anyhow::anyhow!("{}: {e:#}", path.display()))?;
    let def = lns_artifact::sandbox::parse_document(&json).map_err(|e| {
        anyhow::anyhow!("{} is not a valid {}: {e:#}", path.display(), kind.as_str())
    })?;
    let mixin = matches!(def, lns_artifact::sandbox::Document::Mixin(_));
    let def = match def {
        // A connector declares no mixins, so there is nothing to compose and nothing to attribute.
        lns_artifact::sandbox::Document::Connector(def) => {
            render_connector(&def, out)?;
            return Ok(0);
        }
        lns_artifact::sandbox::Document::Sandbox(def)
        | lns_artifact::sandbox::Document::Mixin(def) => def,
    };
    let composed = compose(fs, path.parent().unwrap_or(cwd), cwd, &def, mixins)?;
    render_effective(mixin, &def.name, &composed, out)?;
    Ok(0)
}

/// A composed document, the mixin line each source earned — a published reference has no resolved document to name — and the merge's record of which source decided each entry.
struct Composition {
    spec: lns_artifact::sandbox::SandboxSpec,
    mixins: Vec<String>,
    contributions: Vec<lns_artifact::merge::Contribution>,
}

impl Composition {
    /// One author is not an attribution question, so a render of the document alone names no source.
    fn attribution(&self, block: lns_artifact::merge::Block, key: &str) -> String {
        if self.mixins.is_empty() {
            return String::new();
        }
        self.contributions
            .iter()
            .find(|c| c.block == block && c.key == key)
            .map(crate::run::summary::merged_attribution)
            .unwrap_or_default()
    }
}

/// Every mixin this machine holds merges by §3.3.2 — the document, then what it declares, then each flag — because an offline render can read them all without resolving anything.
fn compose<F: Fs + ?Sized>(
    fs: &F,
    document_dir: &Path,
    invocation_dir: &Path,
    def: &lns_artifact::sandbox::Definition,
    flags: &[String],
) -> Result<Composition> {
    if def.spec.mixins.is_empty() && flags.is_empty() {
        return Ok(Composition {
            spec: def.spec.clone(),
            mixins: Vec::new(),
            contributions: Vec::new(),
        });
    }
    let local = super::mixin_offline::resolve(
        fs,
        &super::mixin_offline::Wanted {
            declared: &def.spec.mixins,
            document_dir,
            flags,
            invocation_dir,
        },
    )?;
    let mut base = def.spec.clone();
    base.mixins = local.declared_keys.clone();
    let sources = lns_artifact::merge::flatten(&base, &local.flag_keys, None, &local.graph)?;
    let merged = lns_artifact::merge::merge(&sources)?;
    Ok(Composition {
        mixins: mixin_lines(&sources, &local.published),
        spec: merged.spec,
        contributions: merged.contributions,
    })
}

/// The merge's own source list is the disclosure, so every merged mixin is listed however deep it was named, once, in merge order; §3.3.1 makes the resolved document's path its identity, a published one has none, and its line says why it is only listed.
fn mixin_lines(
    sources: &[lns_artifact::merge::Source],
    published: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    sources
        .iter()
        .skip(1)
        .map(|source| {
            if published.contains(source.label) {
                format!(
                    "{} (published; not merged, because this render is offline)",
                    source.label
                )
            } else {
                source.label.to_string()
            }
        })
        .collect()
}

/// A connector never reaches here — it renders on its own path — so the kind this takes is the one bit that is left.
fn render_effective<W: Write>(
    mixin: bool,
    name: &str,
    composed: &Composition,
    out: &mut W,
) -> Result<()> {
    let spec = &composed.spec;
    if mixin {
        writeln!(out, "Mixin: {name}")?;
    } else {
        writeln!(out, "Sandbox: {name}")?;
        writeln!(out, "  image:        {}", spec.image)?;
    }
    for mixin in &composed.mixins {
        writeln!(out, "  mixin: {mixin}")?;
    }
    if let Some(command) = &spec.command {
        writeln!(out, "  command:      {command}")?;
    }
    if let Some(user) = &spec.user {
        writeln!(out, "  user:         {user}")?;
    }
    if let Some(workdir) = &spec.workdir {
        writeln!(out, "  workdir:      {workdir}")?;
    }
    for (key, value) in &spec.env {
        writeln!(out, "  env:          {key}={value}")?;
    }
    let mount = lns_artifact::merge::Block::Mount;
    for volume in &spec.volumes {
        let kind = if volume.is_bind() { "bind" } else { "volume" };
        let mode = if volume.read_only() {
            "read-only"
        } else {
            "read-write"
        };
        writeln!(
            out,
            "  mount:        {kind} {} -> {} ({mode}){}",
            volume.source(),
            volume.target,
            composed.attribution(mount, &volume.target)
        )?;
    }
    for fileset in &spec.filesets {
        let source = crate::run::summary::fileset_source_display(fileset);
        let owner = crate::run::summary::fileset_owner_display(fileset.owner);
        writeln!(
            out,
            "  fileset:      {source} -> {} (owner: {owner}){}",
            fileset.guest_path,
            composed.attribution(mount, &fileset.guest_path)
        )?;
    }
    writeln!(
        out,
        "  egress:       {} route(s){}",
        spec.egress.http.len(),
        raw_rule_note(spec.egress.tcp.len())
    )?;
    for tool in &spec.tools {
        writeln!(
            out,
            "  tool: {tool}{}",
            composed.attribution(
                lns_artifact::merge::Block::Tool,
                lns_artifact::merge::tool_name(tool)
            )
        )?;
    }
    Ok(())
}

/// A connector renders by what it serves and the ways it can be connected, because its payload lives in a method rather than beside one (§3.2).
fn render_connector<W: Write>(
    def: &lns_artifact::connector::ConnectorDefinition,
    out: &mut W,
) -> Result<()> {
    writeln!(out, "Connector: {}", def.name)?;
    for pattern in &def.spec.serves {
        writeln!(out, "  serves:       {pattern}")?;
    }
    for method in &def.spec.methods {
        let auth = match &method.auth {
            Some(auth) => auth.kind.as_str(),
            None => "none",
        };
        let name = &method.name;
        let unsupported = if method.is_offerable() {
            ""
        } else {
            " — needs a newer lns"
        };
        writeln!(out, "  method:       {name} (auth: {auth}){unsupported}")?;
        writeln!(
            out,
            "    egress:     {} route(s){}",
            method.egress.http.len(),
            raw_rule_note(method.egress.tcp.len())
        )?;
        for credential in &method.credentials {
            writeln!(out, "    credential: {}", credential.owner())?;
        }
        for fileset in &method.filesets {
            writeln!(out, "    file:       {}", fileset.guest_path)?;
        }
        for (key, value) in &method.env {
            writeln!(out, "    env:        {key}={value}")?;
        }
    }
    Ok(())
}

/// Raw rules are counted separately because they are spliced through uninspected — folding them into the route count would hide that.
fn raw_rule_note(count: usize) -> String {
    match count {
        0 => String::new(),
        n => format!(", {n} raw TCP rule(s)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::artifact::test_support::MapFs;

    fn fake(path: &str, contents: &str) -> MapFs {
        MapFs::with(&[(path, contents)])
    }

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn valid_yaml() -> &'static str {
        "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: ghcr.io/team/base:1\n  tools: [node@22]\n"
    }

    fn inspect_cmd(target: Option<&str>) -> ArtifactCommand {
        ArtifactCommand::Inspect(super::super::InspectArgs {
            reference: target.map(str::to_string),
            mixins: Vec::new(),
            file: None,
        })
    }

    #[test]
    fn is_offline_matches_the_author_verbs_and_local_inspects_only() {
        assert!(is_offline(&ArtifactCommand::Init(super::super::InitArgs {
            kind: DocumentKind::Sandbox,
            file: None,
        })));
        assert!(is_offline(&ArtifactCommand::Validate(
            super::super::ValidateArgs {
                kind: None,
                file: None,
            }
        )));
        assert!(is_offline(&inspect_cmd(None)));
        assert!(is_offline(&inspect_cmd(Some("."))));
        assert!(is_offline(&inspect_cmd(Some("lns.yaml"))));
        assert!(is_offline(&inspect_cmd(Some("../other"))));
        assert!(!is_offline(&inspect_cmd(Some("ghcr.io/team/hermes:1"))));
        assert!(!is_offline(&inspect_cmd(Some("brave_narwhal"))));
        assert!(!is_offline(&ArtifactCommand::Ls(super::super::LsArgs {
            kind: None,
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        })));
    }

    #[test]
    fn init_scaffolds_a_default_definition() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let code = init(&fs, cwd(), DocumentKind::Sandbox, None, &mut out).unwrap();
        assert_eq!(code, 0);
        let written = fs.read_to_string(&yaml_path(cwd())).unwrap();
        assert!(written.contains("kind: sandbox"));
        assert!(written.contains("apiVersion: lns.run/v1"));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("✓ created lns.yaml"), "got: {text}");
        assert!(text.contains("`lns run`"), "got: {text}");
    }

    #[test]
    fn init_refuses_to_clobber_an_existing_definition() {
        let fs = fake("/work/lns.yaml", "keep me");
        let mut out = Vec::new();
        let err = init(&fs, cwd(), DocumentKind::Sandbox, None, &mut out).unwrap_err();
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
        let err = init(&fs, cwd(), DocumentKind::Sandbox, None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
    }

    #[test]
    fn validate_passes_a_well_formed_definition() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), None, None, &mut out).unwrap();
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("is valid"));
    }

    #[test]
    fn validate_refuses_a_secret_shaped_file_in_a_sandbox_fileset() {
        // Validate has to reach push's verdict, and the kind it hands down is what decides it.
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", yaml),
            ("/work/skills/.env", "TOKEN=real"),
        ]);
        let mut out = Vec::new();

        let code = validate(&fs, cwd(), None, None, &mut out).unwrap();

        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("secret-shaped file"), "got: {text}");
    }

    #[test]
    fn validate_file_selector_roots_filesets_at_the_files_directory_and_names_it() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n";
        let fs = MapFs::with(&[
            ("/other/lns.dev.yaml", yaml),
            ("/other/skills/prompts.md", "p"),
        ]);
        let mut out = Vec::new();
        let code = validate(
            &fs,
            cwd(),
            None,
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
            "apiVersion: lns.run/v1\nkind: sandbox\nname: dev\nspec: {}\n",
        );
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), None, Some(Path::new("lns.dev.yaml")), &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("lns.dev.yaml is not valid:"), "got: {text}");
    }

    #[test]
    fn validate_surfaces_a_missing_file() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let err = validate(&fs, cwd(), None, None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("lns init"));
    }

    #[test]
    fn a_missing_variant_file_does_not_hint_lns_init() {
        let fs = MapFs::default();
        let mut out = Vec::new();
        let err =
            validate(&fs, cwd(), None, Some(Path::new("lns.dev.yaml")), &mut out).unwrap_err();
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
        let err = validate(&fs, cwd(), None, None, &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("parsing"));
    }

    #[test]
    fn inspect_local_refuses_a_published_mixin_it_cannot_resolve() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let err = inspect_local(
            &fs,
            cwd(),
            None,
            None,
            &["ghcr.io/acme/obs:2".to_string()],
            &mut out,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("an offline render resolves nothing"),
            "rendering without it would show a composition the file does not describe, and fetching it would make an offline render reach the network; got: {err:#}"
        );
    }

    #[test]
    fn inspect_local_merges_a_mixin_the_user_names_by_path() {
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            (
                "/work/obs/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  tools:\n    - node@22\n",
            ),
        ]);
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("tool: node@22"),
            "the mixin is on this machine, so the composition it makes is renderable without resolving anything; got: {text}"
        );
        assert!(text.contains("mixin: /work/obs/lns.yaml"), "got: {text}");
    }

    #[test]
    fn a_mixin_that_names_itself_is_refused_by_the_merge() {
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            (
                "/work/obs/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  mixins:\n    - ./\n",
            ),
        ]);
        let mut out = Vec::new();
        let err =
            inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap_err();
        assert!(
            format!("{err:#}").contains("reachable from itself"),
            "a cycle has no composition to render; got: {err:#}"
        );
    }

    #[test]
    fn inspect_local_renders_a_connector_by_what_it_serves_and_how_it_connects() {
        let yaml = concat!(
            "apiVersion: lns.run/v1\nkind: connector\nname: stripe\nspec:\n",
            "  serves:\n    - api.stripe.com\n",
            "  methods:\n",
            "    - name: token\n      auth:\n        kind: token\n",
            "      egress:\n        http:\n          - match: api.stripe.com\n            verdict: allow\n",
            "        tcp:\n          - match: api.stripe.com:5432\n            verdict: allow\n",
            "      credentials:\n        - envVar: STRIPE_KEY\n          placeholder: stripe_LNSPLACEHOLDER00000\n",
            "      filesets:\n        - guestPath: /home/agent/.stripe\n          inline:\n            config.json: '{}'\n",
            "      env:\n        STRIPE_REGION: eu\n",
            "    - name: sso\n      auth:\n        kind: oauth_device\n",
            "    - name: open\n      egress:\n        http:\n          - match: api.stripe.com\n            verdict: allow\n",
        );
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        for expected in [
            "Connector: stripe",
            "serves:       api.stripe.com",
            "method:       token (auth: token)",
            "1 route(s), 1 raw TCP rule(s)",
            "credential: STRIPE_KEY",
            "file:       /home/agent/.stripe",
            "env:        STRIPE_REGION=eu",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in: {text}");
        }
        assert!(
            text.contains("method:       sso (auth: oauth_device) — needs a newer lns"),
            "a mechanism this version cannot run is listed and marked, because the document is valid and its other methods still work: {text}"
        );
        assert!(
            text.contains("method:       open (auth: none)"),
            "a method with nothing to sign in to is granted rather than connected, and the card still has to show it: {text}"
        );
        assert!(
            !text.contains("image:"),
            "a connector carries no image, so rendering it as a sandbox would print a field it cannot have: {text}"
        );
    }

    #[test]
    fn a_mixin_that_is_a_sandbox_is_named_as_the_wrong_kind() {
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            ("/work/obs/lns.yaml", valid_yaml()),
        ]);
        let mut out = Vec::new();
        let err =
            inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("/work/obs/lns.yaml is not a mixin"),
            "got: {text}"
        );
    }

    #[test]
    fn a_mixin_path_that_holds_no_document_names_the_path() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let err =
            inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("/work/obs"), "got: {err:#}");
    }

    #[test]
    fn inspect_local_renders_the_effective_definition() {
        let fs = fake("/work/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("image:") && text.contains("ghcr.io/team/base:1"),
            "got: {text}"
        );
        assert!(
            text.contains("egress:") && text.contains("route(s)"),
            "got: {text}"
        );
        assert!(text.contains("tool: node@22"), "got: {text}");
    }

    #[test]
    fn inspect_local_merges_the_mixins_the_definition_layers_on() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  mixins:\n    - ./mixins/postgres-tools/\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", yaml),
            (
                "/work/mixins/postgres-tools/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: postgres-tools\nspec:\n  tools:\n    - node@22\n",
            ),
        ]);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("tool: node@22"),
            "a run merges what the document declares, so a render that only listed it would preview a sandbox nobody boots: {text}"
        );
        assert!(
            text.contains("mixin: /work/mixins/postgres-tools/lns.yaml"),
            "§3.3.1 makes the resolved document the mixin's identity, and that is what the disclosure names: {text}"
        );
    }

    #[test]
    fn a_mixin_a_mixin_names_is_listed_alongside_what_it_merged() {
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            (
                "/work/obs/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  mixins:\n    - ../deep\n",
            ),
            (
                "/work/deep/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: deep\nspec:\n  tools:\n    - python@3.12\n",
            ),
        ]);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("mixin: /work/deep/lns.yaml"),
            "the render exists to disclose what merged, and this mixin's tool merged: {text}"
        );
    }

    #[test]
    fn a_path_both_declared_and_named_by_a_flag_is_listed_once() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  mixins:\n    - ./obs\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", yaml),
            (
                "/work/obs/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  tools:\n    - node@22\n",
            ),
        ]);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.matches("mixin: /work/obs/lns.yaml").count(),
            1,
            "one document is one source, however many spellings reached it: {text}"
        );
    }

    #[test]
    fn a_published_entry_is_listed_where_it_was_declared() {
        let yaml = format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  mixins:\n    - ghcr.io/acme/obs@sha256:{}\n    - ./local\n",
            "a".repeat(64)
        );
        let fs = MapFs::with(&[
            ("/work/lns.yaml", yaml.as_str()),
            (
                "/work/local/lns.yaml",
                "apiVersion: lns.run/v1\nkind: mixin\nname: local\nspec:\n  tools:\n    - node@22\n",
            ),
        ]);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.find("ghcr.io/acme/obs").unwrap() < text.find("/work/local/lns.yaml").unwrap(),
            "the list is the merge order, and the document declared the published entry first: {text}"
        );
    }

    #[test]
    fn each_merged_entry_names_the_source_that_decided_it() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  tools:\n    - node@20\n";
        let mixin = "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  tools:\n    - node@22\n  filesets:\n    - inline:\n        settings.json: '{}'\n      guestPath: /root/.agent/settings\n  volumes:\n    - type: volume\n      source: obs-cache\n      target: /cache\n";
        let fs = MapFs::with(&[("/work/lns.yaml", yaml), ("/work/obs/lns.yaml", mixin)]);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &["./obs".to_string()], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        for line in [
            "tool: node@22  [from /work/obs/lns.yaml, replaced node@20 from the sandbox]",
            "fileset:      inline -> /root/.agent/settings (owner: workload)  [from /work/obs/lns.yaml]",
            "mount:        volume obs-cache -> /cache (read-write)  [from /work/obs/lns.yaml]",
        ] {
            assert!(
                text.contains(line),
                "a reader of a composition must not have to open each source to learn which one put an entry there; missing {line:?} in: {text}"
            );
        }
    }

    #[test]
    fn a_render_of_one_document_alone_names_no_source() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  tools:\n    - node@20\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.contains("[from"),
            "one author is not an attribution question: {text}"
        );
    }

    #[test]
    fn a_published_mixin_the_document_declares_is_listed_rather_than_refused() {
        let yaml = format!(
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  mixins:\n    - ghcr.io/acme/obs@sha256:{}\n",
            "a".repeat(64)
        );
        let fs = fake("/work/lns.yaml", &yaml);
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("(published; not merged, because this render is offline)"),
            "every document written to publish pins its mixins by digest, so refusing the render over one would refuse them all — but a reader must not read the rest as the whole composition: {text}"
        );
    }

    #[test]
    fn a_flag_roots_where_the_user_typed_it_and_a_declared_entry_at_its_own_document() {
        let variant = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev\nspec:\n  image: x:1\n  mixins:\n    - ./obs\n";
        let mixin = |tool: &str| {
            format!(
                "apiVersion: lns.run/v1\nkind: mixin\nname: obs\nspec:\n  tools:\n    - {tool}\n"
            )
        };
        let fs = MapFs::with(&[
            ("/other/lns.dev.yaml", variant),
            ("/other/obs/lns.yaml", &mixin("node@22")),
            ("/work/obs/lns.yaml", &mixin("python@3.12")),
        ]);
        let mut out = Vec::new();
        inspect_local(
            &fs,
            cwd(),
            None,
            Some(Path::new("../other/lns.dev.yaml")),
            &["./obs".to_string()],
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("mixin: /other/obs/lns.yaml")
                && text.contains("mixin: /work/obs/lns.yaml"),
            "§3.3.1 roots a declared entry at the document that named it and a flag where the user typed it, so one spelling reaches two different mixins: {text}"
        );
    }

    #[test]
    fn inspect_local_counts_raw_rules_apart_from_routes() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  egress:\n    http:\n      - match: api.example.test\n        verdict: allow\n    tcp:\n      - match: db.internal:5432\n        verdict: allow\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("1 route(s), 1 raw TCP rule(s)"),
            "a raw splice is not one more inspected route; folding the counts would hide it: {text}"
        );
    }

    #[test]
    fn inspect_local_renders_a_command_when_present() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  command: agent --serve\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("command:      agent --serve")
        );
    }

    #[test]
    fn inspect_local_renders_env_sorted_by_key() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  env:\n    SHELL: /bin/sh\n    FOO: bar\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap();
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
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n",
        );
        let mut out = Vec::new();
        let code = validate(&fs, cwd(), None, None, &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("fileset ./skills"), "got: {text}");
    }

    #[test]
    fn inspect_local_renders_path_and_inline_filesets() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  filesets:\n    - path: ./skills\n      guestPath: /root/.agent/skills\n    - inline:\n        settings.json: '{}'\n      guestPath: /root/.agent/settings\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), Some("."), None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("fileset:      ./skills -> /root/.agent/skills"),
            "got: {text}"
        );
        assert!(
            text.contains("fileset:      inline -> /root/.agent/settings"),
            "got: {text}"
        );
    }

    #[test]
    fn inspect_local_lists_each_declared_tool() {
        let yaml = "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec:\n  image: x:1\n  tools:\n    - node@22\n    - python@3.12\n";
        let fs = fake("/work/lns.yaml", yaml);
        let mut out = Vec::new();
        inspect_local(&fs, cwd(), Some("."), None, &[], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("tool: node@22"), "got: {text}");
        assert!(text.contains("tool: python@3.12"), "got: {text}");
    }

    #[test]
    fn inspect_local_renders_another_directorys_definition_by_path() {
        let fs = fake("/other/lns.yaml", valid_yaml());
        let mut out = Vec::new();
        let code = inspect_local(&fs, cwd(), Some("../other"), None, &[], &mut out).unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("ghcr.io/team/base:1"), "got: {text}");
    }

    #[test]
    fn inspect_file_selector_renders_the_named_definition() {
        let variant = "apiVersion: lns.run/v1\nkind: sandbox\nname: dev\nspec:\n  image: ghcr.io/team/dev:1\n";
        let fs = MapFs::with(&[
            ("/work/lns.yaml", valid_yaml()),
            ("/work/lns.dev.yaml", variant),
        ]);
        let mut out = Vec::new();
        let code = inspect_local(
            &fs,
            cwd(),
            None,
            Some(Path::new("lns.dev.yaml")),
            &[],
            &mut out,
        )
        .unwrap();
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
            &[],
            &mut out,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not both"), "got: {err:#}");
    }

    #[test]
    fn a_file_selector_inspect_is_offline() {
        assert!(is_offline(&ArtifactCommand::Inspect(
            super::super::InspectArgs {
                reference: None,
                mixins: Vec::new(),
                file: Some(std::path::PathBuf::from("lns.dev.yaml")),
            }
        )));
    }

    #[test]
    fn inspect_local_refuses_an_invalid_definition() {
        let fs = fake(
            "/work/lns.yaml",
            "apiVersion: lns.run/v1\nkind: sandbox\nname: hermes\nspec: {}\n",
        );
        let mut out = Vec::new();
        let err = inspect_local(&fs, cwd(), None, None, &[], &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("not a valid sandbox"));
    }
}
