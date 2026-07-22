use std::fmt::Write as _;
use std::io::{BufRead, Write};

use anyhow::{Result, bail};

/// What a pulled-by-reference sandbox will mount into the workload: host binds it declared (resolved against this machine) and author-published filesets.
pub struct PulledMounts<'a> {
    pub reference: &'a str,
    pub binds: &'a [lns_ipc::BindSpec],
    pub filesets: &'a [(String, String)],
}

impl PulledMounts<'_> {
    fn is_empty(&self) -> bool {
        self.binds.is_empty() && self.filesets.is_empty()
    }
}

pub fn confirm_pulled_mounts(
    mounts: &PulledMounts,
    assume_yes: bool,
    interactive: bool,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<()> {
    if mounts.is_empty() || assume_yes {
        return Ok(());
    }
    output.write_all(disclosure(mounts).as_bytes())?;
    if !interactive {
        bail!(
            "{} mounts the paths above into the workload and there is no terminal to confirm — run interactively, or pass --yes to accept them",
            mounts.reference
        );
    }
    write!(output, "Continue? [y/N]: ")?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        Ok(())
    } else {
        bail!("run declined; nothing was mounted or started");
    }
}

fn disclosure(mounts: &PulledMounts) -> String {
    let mut s = String::new();
    writeln!(s, "{} mounts into the workload:", mounts.reference).unwrap();
    for bind in mounts.binds {
        let mode = if bind.read_only {
            "read"
        } else {
            "read and write"
        };
        writeln!(
            s,
            "  Host bind: {} → {} — the workload can {mode} this host directory",
            bind.host_source, bind.target
        )
        .unwrap();
    }
    for (source, mount_path) in mounts.filesets {
        writeln!(
            s,
            "  Fileset:   {source} → {mount_path} — author-published files the workload will read"
        )
        .unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(host_source: &str, read_only: bool) -> lns_ipc::BindSpec {
        lns_ipc::BindSpec {
            host_source: host_source.into(),
            target: "/work".into(),
            read_only,
        }
    }

    fn confirm(
        mounts: &PulledMounts,
        assume_yes: bool,
        interactive: bool,
        answer: &str,
    ) -> (Result<()>, String) {
        let mut input = std::io::Cursor::new(answer.to_string());
        let mut out = Vec::new();
        let r = confirm_pulled_mounts(mounts, assume_yes, interactive, &mut input, &mut out);
        (r, String::from_utf8(out).unwrap())
    }

    fn mounts<'a>(
        binds: &'a [lns_ipc::BindSpec],
        filesets: &'a [(String, String)],
    ) -> PulledMounts<'a> {
        PulledMounts {
            reference: "ghcr.io/team/hermes:1",
            binds,
            filesets,
        }
    }

    #[test]
    fn a_pulled_sandbox_with_no_mounts_never_prompts() {
        let (r, out) = confirm(&mounts(&[], &[]), false, true, "");
        r.unwrap();
        assert!(out.is_empty(), "no mounts must mean no prompt: {out:?}");
    }

    #[test]
    fn yes_flag_accepts_the_mounts_without_prompting() {
        let binds = [bind("/Users/me/proj", false)];
        let (r, out) = confirm(&mounts(&binds, &[]), true, true, "");
        r.unwrap();
        assert!(out.is_empty(), "--yes must skip the prompt: {out:?}");
    }

    #[test]
    fn disclosure_names_each_bind_with_its_resolved_host_path_and_mode() {
        let binds = [bind("/Users/me/proj", false), bind("/Users/me/cfg", true)];
        let (r, out) = confirm(&mounts(&binds, &[]), false, true, "y\n");
        r.unwrap();
        assert!(
            out.contains("ghcr.io/team/hermes:1 mounts into the workload:"),
            "got: {out}"
        );
        assert!(
            out.contains("Host bind: /Users/me/proj → /work — the workload can read and write this host directory"),
            "got: {out}"
        );
        assert!(
            out.contains(
                "Host bind: /Users/me/cfg → /work — the workload can read this host directory"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn disclosure_names_each_fileset_with_its_mount_path() {
        let filesets = [(
            "reg/skills@sha256:abcabcabcabc…".to_string(),
            "/root/.agent/skills".to_string(),
        )];
        let (r, out) = confirm(&mounts(&[], &filesets), false, true, "yes\n");
        r.unwrap();
        assert!(
            out.contains(
                "Fileset:   reg/skills@sha256:abcabcabcabc… → /root/.agent/skills — author-published files the workload will read"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn an_empty_answer_declines_and_the_run_never_starts() {
        let binds = [bind("/Users/me/proj", false)];
        let err = confirm(&mounts(&binds, &[]), false, true, "\n")
            .0
            .unwrap_err();
        assert!(
            err.to_string().contains("declined"),
            "default must be No: {err}"
        );
    }

    #[test]
    fn an_explicit_no_declines() {
        let binds = [bind("/Users/me/proj", false)];
        let err = confirm(&mounts(&binds, &[]), false, true, "n\n")
            .0
            .unwrap_err();
        assert!(err.to_string().contains("declined"), "got: {err}");
    }

    #[test]
    fn no_terminal_fails_closed_and_points_at_the_yes_flag() {
        let filesets = [("reg/skills@sha256:abc".to_string(), "/skills".to_string())];
        let (r, out) = confirm(&mounts(&[], &filesets), false, false, "");
        let err = r.unwrap_err().to_string();
        assert!(err.contains("--yes"), "must name the escape hatch: {err}");
        assert!(
            err.contains("ghcr.io/team/hermes:1"),
            "must name the sandbox: {err}"
        );
        assert!(
            out.contains("Fileset:"),
            "the refusal must still disclose what it refused: {out}"
        );
    }
}
