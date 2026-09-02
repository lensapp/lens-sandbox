use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_policy::host_bind_decisions::{HostBindDecisionStore, SecretDisposition};

pub trait DirScan {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn entries(&self, dir: &Path) -> Vec<String>;
}

pub struct RealDirScan;

impl DirScan for RealDirScan {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn entries(&self, dir: &Path) -> Vec<String> {
        let Ok(read) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        read.flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }
}

pub fn looks_like_secret(name: &str) -> bool {
    lns_artifact::sandbox::looks_like_secret_name(name)
}

fn secret_shaped_top_level(scan: &dyn DirScan, root: &Path) -> Vec<String> {
    let mut found: Vec<String> = scan
        .entries(root)
        .into_iter()
        .filter(|name| looks_like_secret(name))
        .collect();
    found.sort();
    found
}

/// Excluded paths that exist in the bind, dropped with no prompt; a nested entry is honored so an explicit "never expose this" reaches a secret the top-level scan can't see.
fn excluded_paths_present(
    scan: &dyn DirScan,
    root: &Path,
    exclude: &[String],
) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for entry in exclude {
        validate_exclude_entry(entry)?;
        if scan.exists(&root.join(entry)) && !found.contains(entry) {
            found.push(entry.clone());
        }
    }
    found.sort();
    Ok(found)
}

/// An exclude entry is masked at `target/<entry>` in the guest, so an absolute or `..`-bearing entry would escape the bind — refuse it loudly rather than silently expose the file the author listed.
fn validate_exclude_entry(entry: &str) -> Result<()> {
    if entry.starts_with('/') {
        bail!("exclude entry {entry:?} must be relative to the bind, not an absolute path");
    }
    if entry.split('/').any(|seg| seg == "." || seg == "..") {
        bail!("exclude entry {entry:?} must not contain `.` or `..` path segments");
    }
    if entry.contains(lns_ipc::cmdline_unsafe_char) {
        bail!(
            "exclude entry {entry:?} must be free of whitespace, quotes, and control characters, which can't be carried to the guest safely"
        );
    }
    Ok(())
}

pub fn is_excluded(name: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|entry| entry == name)
}

/// One host bind after secret resolution: the secrets to expose (`kept`) and to mask (`dropped`), ready to render in the summary or lower to the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBind {
    pub host_source: String,
    pub target: String,
    pub read_only: bool,
    pub kept: Vec<String>,
    pub dropped: Vec<String>,
    pub excluded: Vec<String>,
}

impl ResolvedBind {
    pub fn to_wire(&self) -> lns_ipc::BindMount {
        lns_ipc::BindMount {
            host_source: self.host_source.clone(),
            target: self.target.clone(),
            read_only: self.read_only,
            dropped_paths: self.dropped.clone(),
            kept_paths: self.kept.clone(),
            excluded_paths: self.excluded.clone(),
        }
    }
}

/// Resolves a KEEP/DROP for every secret-shaped file in each bind (and every excluded path, whatever its shape) — an exclude and a recorded decision drop silently, the rest prompt or default to DROP when there is no terminal.
pub fn resolve_binds(
    specs: &[lns_ipc::BindSpec],
    scan: &dyn DirScan,
    store: &HostBindDecisionStore,
    terminal: &mut dyn crate::terminal::Terminal,
    output: &mut dyn Write,
) -> Result<Vec<ResolvedBind>> {
    let mut decisions = store.load().context("loading host-bind decisions")?;
    let mut recorded = false;
    let mut resolved = Vec::with_capacity(specs.len());
    for spec in specs {
        let root = Path::new(&spec.host_source);
        if !scan.exists(root) {
            if !spec.optional {
                bail!("host path does not exist: {}", spec.host_source);
            }
            writeln!(
                output,
                "lns: skipping optional bind {} — not present on this host",
                spec.host_source
            )?;
            continue;
        }
        if !scan.is_dir(root) {
            bail!(
                "host bind source must be a directory: {} — bind its parent directory instead",
                spec.host_source
            );
        }
        let mut kept = Vec::new();
        let mut dropped = Vec::new();
        for path in excluded_paths_present(scan, root, &spec.exclude)? {
            push_drop(&mut dropped, path)?;
        }
        for name in secret_shaped_top_level(scan, root) {
            if is_excluded(&name, &spec.exclude) {
                continue;
            }
            let key = root.join(&name).to_string_lossy().into_owned();
            let disposition = match decisions.get(&key).copied() {
                Some(d) => d,
                None if terminal.is_available() => {
                    let chosen = prompt_disposition(&key, terminal, output)?;
                    decisions.insert(key.clone(), chosen);
                    recorded = true;
                    chosen
                }
                None => {
                    writeln!(
                        output,
                        "lns: no terminal to ask — dropping secret-shaped file {key} from the bind (run interactively to keep it)"
                    )?;
                    SecretDisposition::Drop
                }
            };
            match disposition {
                SecretDisposition::Keep => kept.push(name),
                SecretDisposition::Drop => push_drop(&mut dropped, name)?,
            }
        }
        resolved.push(ResolvedBind {
            host_source: spec.host_source.clone(),
            target: spec.target.clone(),
            read_only: spec.read_only,
            kept,
            dropped,
            excluded: spec.exclude.clone(),
        });
    }
    if recorded {
        store
            .save(&decisions)
            .context("saving host-bind decisions")?;
    }
    Ok(resolved)
}

/// A dropped path travels to the guest on the kernel cmdline the guest tokenizes; a name with whitespace or a quote would be mis-split or mis-masked, so we refuse it loudly rather than risk exposing the very file the operator chose to drop.
fn push_drop(dropped: &mut Vec<String>, name: String) -> Result<()> {
    if name.contains(lns_ipc::cmdline_unsafe_char) {
        bail!(
            "cannot drop {name:?}: its name contains whitespace, quotes, or control characters, which can't be carried to the guest safely — rename it or move it out of the bind"
        );
    }
    dropped.push(name);
    Ok(())
}

fn prompt_disposition(
    secret_path: &str,
    terminal: &mut dyn crate::terminal::Terminal,
    output: &mut dyn Write,
) -> Result<SecretDisposition> {
    write!(
        output,
        "Host bind: {secret_path} looks like a secret. Expose it to the workload? [k]eep / [D]rop (default): "
    )?;
    output.flush()?;
    let answer = terminal.read_answer()?;
    let answer = answer.trim().to_ascii_lowercase();
    Ok(if answer == "k" || answer == "keep" {
        SecretDisposition::Keep
    } else {
        SecretDisposition::Drop
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::ScriptedTerminal;
    use lns_policy::decision_store::DecisionStore;
    use lns_policy::host_bind_decisions::HostBindDecisionFile;

    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeDir {
        entries: Vec<String>,
        missing: Vec<String>,
    }
    impl DirScan for FakeDir {
        fn exists(&self, path: &Path) -> bool {
            !self.missing.contains(&path.to_string_lossy().into_owned())
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.exists(path)
        }
        fn entries(&self, _dir: &Path) -> Vec<String> {
            self.entries.clone()
        }
    }

    #[derive(Default)]
    struct FakeStore {
        state: Mutex<HostBindDecisionFile>,
        saves: Mutex<usize>,
    }
    impl DecisionStore<SecretDisposition> for FakeStore {
        fn load(&self) -> std::io::Result<HostBindDecisionFile> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn save(&self, state: &HostBindDecisionFile) -> std::io::Result<()> {
            *self.state.lock().unwrap() = state.clone();
            *self.saves.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn bind(src: &str, target: &str) -> lns_ipc::BindSpec {
        lns_ipc::BindSpec {
            host_source: src.into(),
            target: target.into(),
            read_only: false,
            exclude: Vec::new(),
            optional: false,
        }
    }

    fn resolve(
        specs: &[lns_ipc::BindSpec],
        dir: &FakeDir,
        store: &FakeStore,
        interactive: bool,
        answer: &str,
    ) -> (Result<Vec<ResolvedBind>>, String) {
        let mut terminal = if interactive {
            ScriptedTerminal::answering(&[answer])
        } else {
            ScriptedTerminal::absent()
        };
        let mut out = Vec::new();
        let r = resolve_binds(specs, dir, store, &mut terminal, &mut out);
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn looks_like_secret_matches_env_keys_and_known_credential_files() {
        for s in [
            ".env",
            ".env.local",
            "server.pem",
            "tls.key",
            "server.ppk",
            "release.keystore",
            "credentials.json",
            ".npmrc",
            ".netrc",
            ".pypirc",
            ".yarnrc.yml",
            "auth.json",
            "id_rsa",
            "id_ed25519",
            ".ssh",
            ".aws",
            ".kube",
            ".azure",
            ".oci",
            ".docker",
        ] {
            assert!(looks_like_secret(s), "{s} should look like a secret");
        }
    }

    #[test]
    fn looks_like_secret_passes_ordinary_files() {
        for s in ["main.rs", "README.md", "Cargo.toml", "src", "package.json"] {
            assert!(!looks_like_secret(s), "{s} should not look like a secret");
        }
    }

    #[test]
    fn secret_shaped_top_level_returns_sorted_secret_shaped_entries() {
        let dir = FakeDir {
            entries: vec![
                "src".into(),
                ".npmrc".into(),
                "Cargo.toml".into(),
                ".env".into(),
            ],
            ..Default::default()
        };
        let found = secret_shaped_top_level(&dir, Path::new("/proj"));
        assert_eq!(found, vec![".env".to_string(), ".npmrc".to_string()]);
    }

    #[test]
    fn secret_shaped_top_level_is_empty_for_a_clean_directory() {
        let dir = FakeDir {
            entries: vec!["src".into(), "Cargo.toml".into()],
            ..Default::default()
        };
        assert!(secret_shaped_top_level(&dir, Path::new("/proj")).is_empty());
    }

    #[test]
    fn excluded_paths_present_honors_top_level_and_nested_entries_that_exist() {
        let dir = FakeDir::default();
        let found = excluded_paths_present(
            &dir,
            Path::new("/proj"),
            &["notes.txt".to_string(), "packages/api/.env".to_string()],
        )
        .unwrap();
        assert_eq!(
            found,
            vec!["notes.txt".to_string(), "packages/api/.env".to_string()],
            "a nested exclude path is honored, not silently ignored"
        );
    }

    #[test]
    fn excluded_paths_present_skips_a_listed_path_that_is_absent() {
        let dir = FakeDir {
            missing: vec!["/proj/ghost.env".into()],
            ..Default::default()
        };
        let found =
            excluded_paths_present(&dir, Path::new("/proj"), &["ghost.env".to_string()]).unwrap();
        assert!(
            found.is_empty(),
            "an exclude for an absent file is a no-op, not a phantom drop"
        );
    }

    #[test]
    fn excluded_paths_present_names_one_path_once_however_often_it_is_listed() {
        let dir = FakeDir::default();
        let found = excluded_paths_present(
            &dir,
            Path::new("/proj"),
            &[".env".to_string(), ".env".to_string()],
        )
        .unwrap();
        assert_eq!(
            found,
            vec![".env".to_string()],
            "two masks over one guest path would stack"
        );
    }

    #[test]
    fn excluded_paths_present_refuses_an_entry_that_would_escape_the_bind() {
        let dir = FakeDir::default();
        for bad in ["../secrets", "/etc/shadow", "a/../../etc"] {
            let err = excluded_paths_present(&dir, Path::new("/proj"), &[bad.to_string()])
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("exclude entry"),
                "{bad} must be refused loudly: {err}"
            );
        }
    }

    #[test]
    fn is_excluded_matches_exact_names_only() {
        let exclude = vec![".env".to_string()];
        assert!(is_excluded(".env", &exclude));
        assert!(!is_excluded(".env.local", &exclude));
    }

    #[test]
    fn real_dir_scan_reads_entries_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        let scan = RealDirScan;
        assert!(scan.exists(&dir.path().join(".env")));
        assert!(!scan.exists(&dir.path().join("absent")));
        assert!(scan.is_dir(dir.path()));
        assert!(
            !scan.is_dir(&dir.path().join(".env")),
            "a regular file is not a directory the guest can share"
        );
        assert!(scan.entries(dir.path()).contains(&".env".to_string()));
        assert!(scan.entries(Path::new("/no/such/dir")).is_empty());
    }

    fn bind_excluding(src: &str, target: &str, exclude: &[&str]) -> lns_ipc::BindSpec {
        lns_ipc::BindSpec {
            exclude: exclude.iter().map(|e| e.to_string()).collect(),
            ..bind(src, target)
        }
    }

    #[test]
    fn an_exclude_naming_one_path_twice_drops_it_once() {
        let dir = FakeDir {
            entries: vec![".cargo".into()],
            ..Default::default()
        };
        let store = FakeStore::default();

        let (out, prompt) = resolve(
            &[bind_excluding("/proj", "/work", &[".cargo", ".cargo"])],
            &dir,
            &store,
            true,
            "",
        );

        assert_eq!(
            out.unwrap()[0].dropped,
            [".cargo"],
            "two masks over one guest path would stack"
        );
        assert!(
            prompt.is_empty(),
            "an author exclude must not prompt: {prompt:?}"
        );
        assert_eq!(
            *store.saves.lock().unwrap(),
            0,
            "an exclude is the author's rule, not a per-machine decision"
        );
    }

    #[test]
    fn an_exclude_for_a_path_that_is_not_there_is_a_no_op() {
        let dir = FakeDir {
            entries: vec!["src".into()],
            missing: vec!["/proj/.cargo".to_string()],
        };
        let store = FakeStore::default();

        let (out, _) = resolve(
            &[bind_excluding("/proj", "/work", &[".cargo"])],
            &dir,
            &store,
            true,
            "",
        );

        assert!(out.unwrap()[0].dropped.is_empty());
    }

    #[test]
    fn resolve_binds_leaves_a_clean_bind_untouched_and_never_prompts() {
        let dir = FakeDir {
            entries: vec!["src".into(), "Cargo.toml".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, prompt) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "");
        let binds = out.unwrap();
        assert_eq!(binds.len(), 1);
        assert!(binds[0].dropped.is_empty());
        assert!(prompt.is_empty(), "clean bind must not prompt: {prompt:?}");
        assert_eq!(*store.saves.lock().unwrap(), 0);
    }

    #[test]
    fn resolve_binds_keep_exposes_the_file_and_records_the_decision() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, prompt) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "keep\n");
        let binds = out.unwrap();
        assert!(binds[0].dropped.is_empty(), "KEEP must not drop");
        assert_eq!(
            binds[0].kept,
            vec![".env".to_string()],
            "KEEP records exposed"
        );
        assert!(prompt.contains("/proj/.env"), "prompt names the secret");
        assert_eq!(
            store.state.lock().unwrap().get("/proj/.env").copied(),
            Some(SecretDisposition::Keep)
        );
        assert_eq!(*store.saves.lock().unwrap(), 1);
    }

    #[test]
    fn resolve_binds_drop_hides_the_file_and_records_the_decision() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "drop\n");
        assert_eq!(out.unwrap()[0].dropped, vec![".env".to_string()]);
        assert_eq!(
            store.state.lock().unwrap().get("/proj/.env").copied(),
            Some(SecretDisposition::Drop)
        );
    }

    #[test]
    fn resolve_binds_empty_answer_defaults_to_drop() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "\n");
        assert_eq!(out.unwrap()[0].dropped, vec![".env".to_string()]);
    }

    #[test]
    fn resolve_binds_reuses_a_recorded_decision_without_prompting() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        store
            .state
            .lock()
            .unwrap()
            .insert("/proj/.env".into(), SecretDisposition::Keep);
        let (out, prompt) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "");
        assert!(out.unwrap()[0].dropped.is_empty());
        assert!(prompt.is_empty(), "a recorded decision must not re-prompt");
        assert_eq!(*store.saves.lock().unwrap(), 0);
    }

    #[test]
    fn resolve_binds_prompts_only_for_the_newly_appeared_secret() {
        let dir = FakeDir {
            entries: vec![".env".into(), ".npmrc".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        store
            .state
            .lock()
            .unwrap()
            .insert("/proj/.env".into(), SecretDisposition::Keep);
        let (out, prompt) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "keep\n");
        out.unwrap();
        assert!(prompt.contains("/proj/.npmrc"), "must prompt for .npmrc");
        assert!(
            !prompt.contains("/proj/.env"),
            "must not re-prompt the decided .env: {prompt:?}"
        );
    }

    #[test]
    fn resolve_binds_an_exclude_pre_empts_the_prompt_for_a_secret_shaped_path() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, prompt) = resolve(
            &[bind_excluding("/proj", "/work", &[".env"])],
            &dir,
            &store,
            true,
            "",
        );
        assert_eq!(out.unwrap()[0].dropped, vec![".env".to_string()]);
        assert!(
            prompt.is_empty(),
            "the author already decided this one: {prompt:?}"
        );
        assert_eq!(
            *store.saves.lock().unwrap(),
            0,
            "an exclude is not a per-machine decision"
        );
    }

    #[test]
    fn resolve_binds_an_exclude_drops_a_non_secret_shaped_file_too() {
        let dir = FakeDir {
            entries: vec!["notes.txt".into(), "src".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, prompt) = resolve(
            &[bind_excluding("/proj", "/work", &["notes.txt"])],
            &dir,
            &store,
            true,
            "",
        );
        assert_eq!(
            out.unwrap()[0].dropped,
            vec!["notes.txt".to_string()],
            "an exclude drops any listed file, not only secret-shaped ones"
        );
        assert!(prompt.is_empty(), "an exclude never prompts");
    }

    #[test]
    fn resolve_binds_an_exclude_drops_a_nested_path_without_prompting() {
        let dir = FakeDir {
            entries: vec!["packages".into(), "src".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, prompt) = resolve(
            &[bind_excluding("/proj", "/work", &["packages/api/.env"])],
            &dir,
            &store,
            true,
            "",
        );
        assert_eq!(
            out.unwrap()[0].dropped,
            vec!["packages/api/.env".to_string()],
            "a nested exclude entry must drop the file, not silently expose it"
        );
        assert!(prompt.is_empty(), "an exclude never prompts");
    }

    #[test]
    fn resolve_binds_refuses_an_exclude_entry_that_escapes_the_bind() {
        let dir = FakeDir {
            entries: vec!["src".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(
            &[bind_excluding("/proj", "/work", &["../../etc/shadow"])],
            &dir,
            &store,
            true,
            "",
        );
        let err = out.unwrap_err().to_string();
        assert!(err.contains("exclude entry"), "got: {err}");
    }

    #[test]
    fn resolve_binds_non_interactive_defaults_to_drop_and_reports() {
        let dir = FakeDir {
            entries: vec![".env".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, report) = resolve(&[bind("/proj", "/work")], &dir, &store, false, "");
        assert_eq!(out.unwrap()[0].dropped, vec![".env".to_string()]);
        assert!(
            report.contains("/proj/.env"),
            "drop is reported: {report:?}"
        );
        assert_eq!(
            *store.saves.lock().unwrap(),
            0,
            "a fallback drop is not recorded"
        );
    }

    #[test]
    fn resolve_binds_refuses_to_drop_a_secret_whose_name_has_whitespace() {
        let dir = FakeDir {
            entries: vec![".env backup".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "drop\n");
        let err = out.unwrap_err().to_string();
        assert!(err.contains("whitespace"), "got: {err}");
    }

    #[test]
    fn resolve_binds_refuses_to_drop_a_secret_whose_name_has_a_quote() {
        let dir = FakeDir {
            entries: vec![".env\"x".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "drop\n");
        let err = out.unwrap_err().to_string();
        assert!(err.contains("quote"), "got: {err}");
    }

    #[test]
    fn resolve_binds_refuses_a_missing_host_path_before_any_run() {
        let dir = FakeDir {
            missing: vec!["/proj".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(&[bind("/proj", "/work")], &dir, &store, true, "");
        let err = out.unwrap_err().to_string();
        assert!(err.contains("host path does not exist"), "got: {err}");
    }

    #[test]
    fn to_wire_carries_source_target_mode_kept_dropped_and_excluded_paths() {
        let resolved = ResolvedBind {
            host_source: "/proj".into(),
            target: "/work".into(),
            read_only: true,
            kept: vec![".env".into()],
            dropped: vec![".npmrc".into()],
            excluded: vec![".cargo".into()],
        };
        let wire = resolved.to_wire();
        assert_eq!(
            wire,
            lns_ipc::BindMount {
                host_source: "/proj".into(),
                target: "/work".into(),
                read_only: true,
                dropped_paths: vec![".npmrc".into()],
                kept_paths: vec![".env".into()],
                excluded_paths: vec![".cargo".into()],
            }
        );
    }

    #[test]
    fn an_exclude_the_guest_cmdline_cannot_carry_is_refused_rather_than_dropped_silently() {
        let dir = FakeDir {
            entries: vec!["src".into()],
            ..Default::default()
        };
        let store = FakeStore::default();
        let (out, _) = resolve(
            &[bind_excluding("/proj", "/work", &["my notes"])],
            &dir,
            &store,
            true,
            "",
        );
        let err = out.unwrap_err().to_string();
        assert!(
            err.contains("exclude entry") && err.contains("whitespace"),
            "an exclude reaches the guest on the kernel cmdline: {err}"
        );
    }
}
