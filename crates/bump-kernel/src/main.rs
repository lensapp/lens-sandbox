use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod operations;

use operations::{
    CommitType, KERNELS_TOML, Variant, back_fill_manifest, bump_manifest, find_workspace_root_from,
    load_compute_results, render_show,
};

#[derive(Parser, Debug)]
#[command(
    name = "bump-kernel",
    about = "Kernel-pin tooling for lns-cli.",
    long_about = "Three subcommands: `bump` opens a PR to bump the guest kernel \
                  pin (operator-facing); `show` emits kernels.toml fields in \
                  GITHUB_OUTPUT format (used by CI); `back-fill` aggregates \
                  per-arch compute results into kernels.toml (used by CI). See \
                  runbooks/kernel-bump.md for the full operator flow."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Open a PR to bump the lns guest kernel pin.")]
    Bump(BumpArgs),
    #[command(about = "Emit kernels.toml fields in GITHUB_OUTPUT format.")]
    Show(ShowArgs),
    #[command(about = "Aggregate per-arch compute-results JSON into kernels.toml.")]
    BackFill(BackFillArgs),
}

#[derive(Args, Debug)]
struct BumpArgs {
    #[arg(
        long,
        help = "Kata Containers release tag (e.g. 3.31.0); validated against the upstream releases endpoint."
    )]
    kata_version: String,

    #[arg(
        long,
        value_enum,
        default_value_t = Variant::Mainline,
        help = "Which vmlinuz variant to publish (mainline is the standard non-TEE kernel; others are for confidential computing)."
    )]
    variant: Variant,

    #[arg(
        long,
        value_enum,
        default_value_t = CommitType::Feat,
        help = "Conventional-commit type for the PR subject; release-please uses this to decide the version bump."
    )]
    commit_type: CommitType,

    #[arg(
        long,
        help = "Validate and show the planned change without touching git or opening a PR; restores kernels.toml to HEAD on exit."
    )]
    dry_run: bool,

    #[arg(long, short, help = "Skip the interactive \"Continue? [y/N]\" prompt.")]
    yes: bool,
}

#[derive(Args, Debug)]
struct ShowArgs {
    #[arg(
        long,
        help = "If set, also emit `expected_sha=<sha>` for this arch; errors if the manifest has no SHA for it."
    )]
    arch: Option<String>,

    #[arg(
        long,
        help = "Path to kernels.toml; defaults to `crates/lns-service/kernels.toml` in the workspace root."
    )]
    manifest: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct BackFillArgs {
    #[arg(
        long,
        default_value = ".compute-results",
        help = "Directory containing per-arch result JSON files (`<arch>.json`)."
    )]
    results_dir: PathBuf,

    #[arg(
        long,
        help = "Path to kernels.toml to rewrite; defaults to `crates/lns-service/kernels.toml` in the workspace root."
    )]
    manifest: Option<PathBuf>,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Bump(args) => cmd_bump(args),
        Commands::Show(args) => cmd_show(args),
        Commands::BackFill(args) => cmd_back_fill(args),
    }
}

fn resolve_manifest(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let workspace = find_workspace_root_from(&std::env::current_dir()?).context(
        "must be run from a lens-sandbox checkout (couldn't find workspace Cargo.toml) — \
         pass --manifest to override",
    )?;
    Ok(workspace.join(KERNELS_TOML))
}

fn cmd_bump(cli: BumpArgs) -> Result<()> {
    let workspace = find_workspace_root_from(&std::env::current_dir()?)
        .context("must be run from a lens-sandbox checkout (couldn't find workspace Cargo.toml)")?;

    validate_kata_release_exists(&cli.kata_version)?;
    require_gh_installed()?;
    let starting_branch = git_current_branch(&workspace)?;
    git_require_clean(&workspace)?;

    let branch = format!("chore/bump-kernel-{}", cli.kata_version);
    if !cli.dry_run {
        git_require_branch_absent(&workspace, &branch)?;
    }

    let manifest_path = workspace.join(KERNELS_TOML);
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;

    let subject = format!(
        "{}(lns): bump guest kernel to Kata {}",
        cli.commit_type.as_str(),
        cli.kata_version,
    );

    println!("==> Plan:");
    println!("       kata_version       3.30.0   -> {}", cli.kata_version);
    println!(
        "       kernel_variant     mainline -> {}",
        cli.variant.as_str()
    );
    println!("       kernel_filename    ...      -> \"pending\"  (CI back-fills)");
    println!("       published_version  ...      -> \"pending\"  (CI back-fills)");
    println!("       [current.sha256].* cleared  -> \"\"         (CI back-fills)");
    println!(
        "       [current.kata_bundle_sha256] -> sha256 of each upstream bundle (computed now)"
    );
    println!();
    println!("==> Commit subject: {subject}");
    println!(
        "       release-please version bump: {}",
        cli.commit_type.version_bump_hint(),
    );
    println!();
    println!("==> Branch:  {branch}");
    println!("==> Base:    {starting_branch}");
    println!();

    if !cli.yes && !cli.dry_run && !confirm("Continue? [y/N] ")? {
        println!("Aborted.");
        return Ok(());
    }

    let bundle_shas = resolve_bundle_shas(&cli.kata_version, cli.dry_run)?;
    let updated = bump_manifest(
        &original,
        &cli.kata_version,
        cli.variant.as_str(),
        &bundle_shas,
    )?;

    std::fs::write(&manifest_path, &updated)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    if cli.dry_run {
        println!();
        println!("==> --dry-run: diff vs HEAD:");
        let _ = Command::new("git")
            .current_dir(&workspace)
            .args([
                "--no-pager",
                "diff",
                "--no-color",
                "--",
                "crates/lns-service/kernels.toml",
            ])
            .status();
        std::fs::write(&manifest_path, &original)
            .with_context(|| format!("restoring {}", manifest_path.display()))?;
        println!();
        println!("==> --dry-run: manifest restored. No branch, commit, or PR created.");
        return Ok(());
    }

    git_create_branch(&workspace, &branch)?;
    git_add(&workspace, "crates/lns-service/kernels.toml")?;
    git_commit(&workspace, &subject)?;
    git_push(&workspace, &branch)?;
    let pr_url = gh_pr_create(&workspace, &subject, &cli, &starting_branch)?;

    println!();
    println!("==> PR opened: {pr_url}");
    println!("==> What happens next:");
    println!("     1. CI's `publish-kernel` workflow downloads Kata, verifies each bundle");
    println!("        against the kata_bundle_sha256 just pinned, resolves the variant,");
    println!("        computes per-arch sha256s, and commits the values back to your PR");
    println!("        as `chore(kernel): fill provenance and shas for ...`.");
    println!("     2. After the bot commit appears, click \"Re-run all jobs\" in the PR's");
    println!("        Checks tab (GITHUB_TOKEN-authored commits don't auto-retrigger CI).");
    println!("     3. Review the back-filled manifest + bot SHAs, then merge. release-please");
    println!("        will roll the change into the next lns-cli release.");
    println!("==> See `runbooks/kernel-bump.md` for the full flow.");
    Ok(())
}

fn cmd_show(args: ShowArgs) -> Result<()> {
    let manifest_path = resolve_manifest(args.manifest.as_deref())?;
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let lines = render_show(&raw, args.arch.as_deref())?;
    print!("{lines}");
    Ok(())
}

fn cmd_back_fill(args: BackFillArgs) -> Result<()> {
    let manifest_path = resolve_manifest(args.manifest.as_deref())?;
    let original = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let results = load_compute_results(&args.results_dir)?;
    let updated = back_fill_manifest(&original, &results)?;
    std::fs::write(&manifest_path, &updated)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    let archs: Vec<&str> = results.iter().map(|r| r.arch.as_str()).collect();
    println!(
        "updated {}: kernel_filename={} published_version={} archs={:?}",
        manifest_path.display(),
        results[0].kernel_filename,
        results[0].published_version,
        archs,
    );
    Ok(())
}

const KATA_BUNDLE_ARCHES: [&str; 2] = ["arm64", "amd64"];

fn resolve_bundle_shas(kata_version: &str, dry_run: bool) -> Result<BTreeMap<String, String>> {
    if dry_run {
        println!();
        println!(
            "==> --dry-run: skipping the ~1.3 GB bundle download; kata_bundle_sha256 shown as a placeholder."
        );
        return Ok(KATA_BUNDLE_ARCHES
            .iter()
            .map(|a| {
                (
                    a.to_string(),
                    "<computed-from-bundle-download-on-a-real-bump>".to_string(),
                )
            })
            .collect());
    }
    let mut shas = BTreeMap::new();
    for kata_arch in KATA_BUNDLE_ARCHES {
        let url = kata_bundle_url(kata_version, kata_arch);
        println!("==> Downloading {url}");
        println!("       (~670 MB; hashing in-stream to pin kata_bundle_sha256.{kata_arch})");
        let sha = curl_sha256(&url)?;
        println!("       {kata_arch} sha256 = {sha}");
        shas.insert(kata_arch.to_string(), sha);
    }
    Ok(shas)
}

fn kata_bundle_url(kata_version: &str, kata_arch: &str) -> String {
    format!(
        "https://github.com/kata-containers/kata-containers/releases/download/\
         {kata_version}/kata-static-{kata_version}-{kata_arch}.tar.zst"
    )
}

fn curl_sha256(url: &str) -> Result<String> {
    let mut child = Command::new("curl")
        .args(["-L", "--fail", "--silent", "--show-error", url])
        .stdout(Stdio::piped())
        .spawn()
        .context("invoking curl to download the Kata bundle (is curl on PATH?)")?;
    let mut stdout = child
        .stdout
        .take()
        .context("capturing curl stdout for hashing")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1 << 16];
    loop {
        let n = stdout
            .read(&mut buf)
            .context("reading Kata bundle bytes from curl")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let status = child.wait().context("waiting on curl")?;
    ensure!(status.success(), "curl failed downloading {url} ({status})");
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_kata_release_exists(version: &str) -> Result<()> {
    let url = format!("https://github.com/kata-containers/kata-containers/releases/tag/{version}");
    let out = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "10",
            "--head",
            "-L",
            &url,
        ])
        .output()
        .context("invoking curl to validate Kata version (is curl on PATH?)")?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if code != "200" {
        bail!(
            "Kata release {version} not found upstream — HEAD {url} returned {code}. \
             Double-check the version against https://github.com/kata-containers/kata-containers/releases."
        );
    }
    Ok(())
}

fn require_gh_installed() -> Result<()> {
    let out = Command::new("gh").arg("--version").output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!(
            "the `gh` CLI is required to open the PR but isn't on PATH or isn't authenticated. \
             Install it (`brew install gh` on macOS, `apt install gh` on Debian/Ubuntu) and run \
             `gh auth login`."
        ),
    }
}

fn git_current_branch(dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("running `git rev-parse --abbrev-ref HEAD`")?;
    ensure!(out.status.success(), "failed to read current git branch");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_require_clean(dir: &Path) -> Result<()> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["status", "--porcelain"])
        .output()
        .context("running `git status --porcelain`")?;
    ensure!(out.status.success(), "git status failed");
    if !out.stdout.is_empty() {
        bail!(
            "git working tree is not clean — commit or stash first:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    Ok(())
}

fn git_require_branch_absent(dir: &Path, branch: &str) -> Result<()> {
    let local = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--verify", "--quiet", branch])
        .output()?;
    if local.status.success() {
        bail!(
            "branch {branch} already exists locally — \
             delete with `git branch -D {branch}` and rerun, \
             or finish the existing bump."
        );
    }
    let remote = Command::new("git")
        .current_dir(dir)
        .args(["ls-remote", "--exit-code", "--heads", "origin", branch])
        .output()?;
    if remote.status.success() {
        bail!(
            "branch {branch} already exists on origin — \
             a bump is already in progress. Either continue that PR, \
             or delete the remote branch (`git push origin --delete {branch}`) \
             and rerun."
        );
    }
    Ok(())
}

fn git_create_branch(dir: &Path, branch: &str) -> Result<()> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["checkout", "-b", branch])
        .status()?;
    ensure!(status.success(), "git checkout -b {branch} failed");
    Ok(())
}

fn git_add(dir: &Path, path: &str) -> Result<()> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["add", path])
        .status()?;
    ensure!(status.success(), "git add {path} failed");
    Ok(())
}

fn git_commit(dir: &Path, subject: &str) -> Result<()> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["commit", "-m", subject])
        .status()?;
    ensure!(status.success(), "git commit failed");
    Ok(())
}

fn git_push(dir: &Path, branch: &str) -> Result<()> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["push", "-u", "origin", branch])
        .status()?;
    ensure!(status.success(), "git push failed");
    Ok(())
}

fn gh_pr_create(dir: &Path, subject: &str, cli: &BumpArgs, base: &str) -> Result<String> {
    let body = format!(
        "Bumps the lns guest kernel to **Kata {kata}** ({variant} variant).\n\
         \n\
         ## What CI will do on this PR\n\
         \n\
         The `publish-kernel` workflow runs automatically:\n\
         \n\
         1. Validates the Kata release tarball exists upstream.\n\
         2. Refuses if the CDN origin already has an artifact at the destination URL.\n\
         3. Downloads Kata's static bundle, verifies it against the \
            `kata_bundle_sha256` pinned in this PR, resolves the `vmlinuz-*` matching \
            the `{variant}` variant, conditionally gunzips (aarch64 only), and computes \
            sha256 for each arch.\n\
         4. Commits provenance + per-arch SHAs back to this PR branch as \
            `chore(kernel): fill provenance and shas for ...`.\n\
         \n\
         ## What you do\n\
         \n\
         1. **Wait for the bot back-fill commit** (~3-5 min).\n\
         2. **Click \"Re-run all jobs\"** in the Checks tab to retrigger CI on the \
            back-filled commit (GITHUB_TOKEN-authored pushes do not retrigger workflows).\n\
         3. **Review** the back-filled manifest + SHAs (cross-check against the \
            workflow run's job summary).\n\
         4. **Merge.** On merge the `publish` job uploads the bytes to the CDN \
            origin under the canonical `lns-kernel-<version>-<arch>` URLs.\n\
         \n\
         release-please will roll the change into the next lns-cli release.\n\
         \n\
         See `runbooks/kernel-bump.md` for the full procedure.",
        kata = cli.kata_version,
        variant = cli.variant.as_str(),
    );
    let out = Command::new("gh")
        .current_dir(dir)
        .args([
            "pr", "create", "--title", subject, "--body", &body, "--base", base,
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "gh pr create failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().lock().read_line(&mut buf)?;
    let answer = buf.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}
