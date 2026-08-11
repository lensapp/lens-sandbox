use std::fmt::Write as _;
use std::io::{BufRead, Write};

use anyhow::{Result, bail};

pub struct PulledEffects<'a> {
    pub reference: &'a str,
    pub binds: &'a [lns_ipc::BindSpec],
    pub volumes: &'a [lns_ipc::VolumeMount],
    pub filesets: &'a [crate::run::summary::FilesetSummary],
    pub tools: &'a [String],
}

impl PulledEffects<'_> {
    fn is_empty(&self) -> bool {
        self.binds.is_empty()
            && self.volumes.is_empty()
            && self.filesets.is_empty()
            && self.tools.is_empty()
    }
}

/// The disclosure covers what the pulled artifact itself will mount: author-declared mounts that survived the consumer's -v overrides, never the consumer's own entries.
pub fn artifact_declared_mounts(
    resolved_mounts: &[lns_ipc::MountSpec],
    consumer_targets: &[String],
) -> (Vec<lns_ipc::VolumeMount>, Vec<lns_ipc::BindSpec>) {
    let declared: Vec<lns_ipc::MountSpec> = resolved_mounts
        .iter()
        .filter(|mount| {
            !consumer_targets
                .iter()
                .any(|target| target == mount.target())
        })
        .cloned()
        .collect();
    crate::cli::split_mounts(&declared)
}

pub fn confirm_pulled_effects(
    effects: &PulledEffects,
    assume_yes: bool,
    interactive: bool,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<()> {
    if effects.is_empty() || assume_yes {
        return Ok(());
    }
    output.write_all(disclosure(effects).as_bytes())?;
    if !interactive {
        bail!(
            "{} declares the effects above and there is no terminal to confirm — run interactively, or pass --yes to accept them",
            effects.reference
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
        bail!("declined; nothing was installed, mounted, or started");
    }
}

fn disclosure(effects: &PulledEffects) -> String {
    let mut s = String::new();
    writeln!(s, "{} declares these effects:", effects.reference).unwrap();
    for bind in effects.binds {
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
    for volume in effects.volumes {
        let mode = if volume.read_only {
            "read"
        } else {
            "read and write"
        };
        writeln!(
            s,
            "  Volume:    {} → {} — the workload can {mode} this machine's persistent volume",
            volume.name, volume.target
        )
        .unwrap();
    }
    for fileset in effects.filesets {
        let mode = if fileset.owner == "workload" {
            "read and write"
        } else {
            "read"
        };
        let provenance = if fileset.from_host {
            "a file read from this machine at launch, which the workload can"
        } else {
            "author-published files the workload can"
        };
        writeln!(
            s,
            "  Fileset:   {} → {} — {provenance} {mode} (owner: {})",
            fileset.source, fileset.mount_path, fileset.owner
        )
        .unwrap();
    }
    for tool in effects.tools {
        writeln!(
            s,
            "  Tool:       {tool} — its installer runs as root in a disposable microVM with unrestricted network access"
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
            exclude: Vec::new(),
            optional: false,
        }
    }

    fn confirm(
        effects: &PulledEffects,
        assume_yes: bool,
        interactive: bool,
        answer: &str,
    ) -> (Result<()>, String) {
        let mut input = std::io::Cursor::new(answer.to_string());
        let mut out = Vec::new();
        let r = confirm_pulled_effects(effects, assume_yes, interactive, &mut input, &mut out);
        (r, String::from_utf8(out).unwrap())
    }

    fn fileset(source: &str, mount_path: &str, owner: &str) -> crate::run::summary::FilesetSummary {
        crate::run::summary::FilesetSummary {
            source: source.into(),
            mount_path: mount_path.into(),
            owner: owner.into(),
            from_host: false,
        }
    }

    fn volume(name: &str, read_only: bool) -> lns_ipc::VolumeMount {
        lns_ipc::VolumeMount {
            name: name.into(),
            target: "/data".into(),
            read_only,
        }
    }

    fn mounts<'a>(
        binds: &'a [lns_ipc::BindSpec],
        filesets: &'a [crate::run::summary::FilesetSummary],
    ) -> PulledEffects<'a> {
        volume_mounts(binds, &[], filesets)
    }

    fn volume_mounts<'a>(
        binds: &'a [lns_ipc::BindSpec],
        volumes: &'a [lns_ipc::VolumeMount],
        filesets: &'a [crate::run::summary::FilesetSummary],
    ) -> PulledEffects<'a> {
        PulledEffects {
            reference: "ghcr.io/team/hermes:1",
            binds,
            volumes,
            filesets,
            tools: &[],
        }
    }

    fn named_spec(name: &str, target: &str) -> lns_ipc::MountSpec {
        lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
            name: name.into(),
            target: target.into(),
            read_only: false,
        })
    }

    fn bind_spec(source: &str, target: &str) -> lns_ipc::MountSpec {
        lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
            host_source: source.into(),
            target: target.into(),
            read_only: false,
            exclude: Vec::new(),
            optional: false,
        })
    }

    #[test]
    fn a_consumer_override_of_a_declared_target_is_not_disclosed() {
        let resolved = [bind_spec("/tmp/safe", "/workspace")];
        let (volumes, binds) = artifact_declared_mounts(&resolved, &["/workspace".to_string()]);
        assert!(
            volumes.is_empty() && binds.is_empty(),
            "the prompt must not name a declared mount the consumer's -v override dropped"
        );
    }

    #[test]
    fn consumer_added_mounts_are_not_disclosed_but_declared_survivors_are() {
        let resolved = [
            named_spec("cache", "/cache"),
            bind_spec("/tmp/extra", "/extra"),
        ];
        let (volumes, binds) = artifact_declared_mounts(&resolved, &["/extra".to_string()]);
        assert_eq!(
            volumes,
            vec![lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/cache".into(),
                read_only: false,
            }],
            "a declared mount the consumer did not touch stays disclosed"
        );
        assert!(
            binds.is_empty(),
            "the consumer's own -v mount needs no consent prompt"
        );
    }

    #[test]
    fn without_consumer_mounts_every_declared_mount_is_disclosed() {
        let resolved = [
            bind_spec("/Users/me/proj", "/workspace"),
            named_spec("cache", "/cache"),
        ];
        let (volumes, binds) = artifact_declared_mounts(&resolved, &[]);
        assert_eq!(binds.len(), 1);
        assert_eq!(volumes.len(), 1);
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
            out.contains("ghcr.io/team/hermes:1 declares these effects:"),
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
    fn disclosure_names_each_fileset_with_its_mount_path_and_access_mode() {
        let filesets = [
            fileset(
                "reg/skills@sha256:abcabcabcabc…",
                "/root/.agent/skills",
                "workload",
            ),
            fileset("inline", "/etc/agent", "root"),
        ];
        let (r, out) = confirm(&mounts(&[], &filesets), false, true, "yes\n");
        r.unwrap();
        assert!(
            out.contains(
                "Fileset:   reg/skills@sha256:abcabcabcabc… → /root/.agent/skills — author-published files the workload can read and write (owner: workload)"
            ),
            "got: {out}"
        );
        assert!(
            out.contains(
                "Fileset:   inline → /etc/agent — author-published files the workload can read (owner: root)"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn disclosure_names_each_named_volume_with_its_mode() {
        let volumes = [volume("home", false), volume("cfg", true)];
        let (r, out) = confirm(&volume_mounts(&[], &volumes, &[]), false, true, "y\n");
        r.unwrap();
        assert!(
            out.contains(
                "Volume:    home → /data — the workload can read and write this machine's persistent volume"
            ),
            "got: {out}"
        );
        assert!(
            out.contains(
                "Volume:    cfg → /data — the workload can read this machine's persistent volume"
            ),
            "got: {out}"
        );
    }

    #[test]
    fn a_named_volume_alone_is_enough_to_prompt() {
        let volumes = [volume("home", false)];
        let err = confirm(&volume_mounts(&[], &volumes, &[]), false, true, "\n")
            .0
            .unwrap_err();
        assert!(
            err.to_string().contains("declined"),
            "an author-named volume must not attach without consent: {err}"
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
        let filesets = [fileset("reg/skills@sha256:abc", "/skills", "workload")];
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

    #[test]
    fn a_publisher_declared_tool_requires_consent_and_discloses_what_executes() {
        let tools = ["node@22".to_string()];
        let effects = PulledEffects {
            reference: "ghcr.io/team/hermes:1",
            binds: &[],
            volumes: &[],
            filesets: &[],
            tools: &tools,
        };

        let mut input = std::io::Cursor::new("n\n");
        let mut out = Vec::new();
        let err = confirm_pulled_effects(&effects, false, true, &mut input, &mut out).unwrap_err();
        let out = String::from_utf8(out).unwrap();

        assert!(err.to_string().contains("declined"), "got: {err}");
        assert!(out.contains("Tool:       node@22"), "got: {out}");
        assert!(
            out.contains("installer runs as root in a disposable microVM"),
            "got: {out}"
        );
        assert!(out.contains("unrestricted network access"), "got: {out}");
    }

    #[test]
    fn a_tool_free_pulled_sandbox_still_needs_no_prompt() {
        let effects = PulledEffects {
            reference: "ghcr.io/team/hermes:1",
            binds: &[],
            volumes: &[],
            filesets: &[],
            tools: &[],
        };

        let mut input = std::io::Cursor::new("");
        let mut out = Vec::new();
        confirm_pulled_effects(&effects, false, false, &mut input, &mut out).unwrap();

        assert!(out.is_empty());
    }
}
