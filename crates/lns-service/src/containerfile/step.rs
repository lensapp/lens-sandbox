//! What one build step boots with, and the one line the run summary owes an approver.

use anyhow::{Context, Result};
use lns_ipc::RunImageArgs;

use super::executor::RunStep;

/// A build step is the document's own run with one instruction in place of the workload: its egress
/// and its credentials decide what the instruction may reach, and nothing the document says about
/// the boot — its command, its workdir, its scripts, its mounts, its ports — reaches a build.
pub(crate) fn args_for(
    base: &RunImageArgs,
    definition: &str,
    step: &RunStep,
) -> Result<RunImageArgs> {
    let mut document: serde_json::Value =
        serde_json::from_str(definition).context("reading the document a build step boots from")?;
    let spec = document
        .get_mut("spec")
        .and_then(serde_json::Value::as_object_mut)
        .context("the document a build step boots from declares no spec")?;
    spec.insert("image".into(), serde_json::json!(step.parent));
    // Declared rather than passed as a flag: a name the flag carries is resolved against this
    // host's nobody uid, where a document's own `user` is resolved by the guest's passwd.
    spec.insert("user".into(), serde_json::json!(step.user));
    for boot_only in ["command", "workdir", "scripts", "volumes", "ports"] {
        spec.remove(boot_only);
    }

    let mut args = base.clone();
    args.definition = Some(serde_json::to_string(&document)?);
    args.image = None;
    args.resolved_image = None;
    args.cmd = step.argv.clone();
    args.env = step.env.clone();
    args.entrypoint = Some(String::new());
    args.workdir = Some(step.workdir.clone());
    args.sandbox_user = None;
    args.sandbox_uid = None;
    args.name = None;
    args.tty = false;
    args.stdin = false;
    args.detached = false;
    args.auto_remove = false;
    args.published_ports = Vec::new();
    args.volumes = Vec::new();
    args.binds = Vec::new();
    Ok(args)
}

/// What was built, for which architecture, and the digest it became.
pub(crate) fn built_line(label: &str, reference: &str, outside_the_gate: bool) -> String {
    let line = format!(
        "built from {label}, {}, {}",
        crate::image::want_arch(),
        reference
            .split_once('@')
            .map(|(_, digest)| digest)
            .unwrap_or(reference),
    );
    match outside_the_gate {
        true => format!(
            "{line} — {}",
            lns_artifact::image_index::BUILT_OUTSIDE_THE_GATE
        ),
        false => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> String {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "sandbox",
            "name": "agent",
            "spec": {
                "image": "./image",
                "command": "agent --serve",
                "workdir": "/workspace",
                "user": "sandbox",
                "scripts": [{ "when": "pre-start", "run": "install-everything" }],
                "volumes": [{ "type": "bind", "source": ".", "target": "/workspace" }],
                "ports": [{ "container": 8080 }],
                "egress": { "http": [{ "match": "registry.npmjs.org", "verdict": "allow" }] },
                "credentials": [{ "envVar": "SOME_TOKEN" }],
                "filesets": [{ "path": "./skills", "guestPath": "/opt/skills" }],
            },
        })
        .to_string()
    }

    fn base() -> RunImageArgs {
        RunImageArgs {
            build_engine: lns_ipc::BuildEngine::default(),
            image: Some("./image".into()),
            resolved_image: Some("./image".into()),
            mixins: vec!["./mixins/tools".into()],
            composed_mixins: vec!["./mixins/tools".into()],
            name: Some("agent".into()),
            cpus: 2,
            mem: 2048,
            cpus_explicit: false,
            mem_explicit: false,
            cpus_config: None,
            mem_config: None,
            sandbox_user: Some("sandbox".into()),
            sandbox_uid: Some(65534),
            entrypoint: None,
            hostname: None,
            cmd: vec!["agent".into(), "--serve".into()],
            env: vec!["MODE=research".into()],
            workdir: Some("/workspace".into()),
            debug: false,
            tty: true,
            stdin: true,
            initial_winsize: Some((24, 80)),
            detached: true,
            published_ports: vec![lns_ipc::PortPublish {
                host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                host_port: 8080,
                container_port: 8080,
                protocol: lns_ipc::Protocol::Tcp,
            }],
            volumes: Vec::new(),
            binds: Vec::new(),
            auto_remove: true,
            verify_sandbox: false,
            definition: Some(document()),
            definition_dir: Some("/home/dev/agent".into()),
            authored_egress: None,
            packed_filesets: Vec::new(),
            denied_host_paths: Vec::new(),
        }
    }

    fn step() -> RunStep {
        RunStep {
            parent: "lns-build.local/built@sha256:parent".into(),
            argv: vec!["/bin/sh".into(), "-c".into(), "npm i -g claude".into()],
            env: vec!["CLAUDE_CODE_VERSION=2.1.263".into()],
            user: "node".into(),
            workdir: "/srv".into(),
            line: 4,
        }
    }

    fn stepped() -> RunImageArgs {
        args_for(&base(), &document(), &step()).expect("this document boots a build step")
    }

    fn spec_of(args: &RunImageArgs) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(args.definition.as_deref().unwrap()).unwrap()
            ["spec"]
            .clone()
    }

    #[test]
    fn the_step_boots_the_image_the_instruction_before_it_produced() {
        let args = stepped();
        assert_eq!(
            spec_of(&args)["image"],
            "lns-build.local/built@sha256:parent"
        );
        assert_eq!(
            args.image, None,
            "the path form must not reach a build step"
        );
        assert_eq!(args.resolved_image, None);
    }

    #[test]
    fn the_instruction_is_what_the_step_runs_and_the_image_s_entrypoint_is_not() {
        let args = stepped();
        assert_eq!(args.cmd, step().argv);
        assert_eq!(args.env, step().env);
        assert_eq!(args.workdir, Some("/srv".to_string()));
        assert_eq!(
            args.entrypoint,
            Some(String::new()),
            "a RUN is the whole command, so the base image's entrypoint must not be prepended",
        );
    }

    #[test]
    fn the_user_the_instructions_set_is_declared_and_not_flagged() {
        let args = stepped();
        assert_eq!(spec_of(&args)["user"], "node");
        assert_eq!(args.sandbox_user, None);
        assert_eq!(args.sandbox_uid, None);
    }

    #[test]
    fn nothing_the_document_says_about_the_boot_reaches_a_build() {
        let args = stepped();
        let spec = spec_of(&args);
        for boot_only in ["command", "workdir", "scripts", "volumes", "ports"] {
            assert_eq!(
                spec.get(boot_only),
                None,
                "{boot_only} describes the run, not the build",
            );
        }
        assert!(args.published_ports.is_empty());
        assert!(args.volumes.is_empty());
        assert!(args.binds.is_empty());
        assert_eq!(args.name, None, "a build step is nobody's named run");
    }

    #[test]
    fn what_the_document_says_a_fetch_may_do_is_what_the_build_is_held_to() {
        let spec = spec_of(&stepped());
        assert_eq!(
            spec["egress"]["http"][0]["match"], "registry.npmjs.org",
            "a RUN is decided by the document's own rules",
        );
        assert_eq!(spec["credentials"][0]["envVar"], "SOME_TOKEN");
        assert_eq!(spec["filesets"][0]["guestPath"], "/opt/skills");
    }

    #[test]
    fn a_build_step_is_never_a_terminal_and_never_detached() {
        let args = stepped();
        assert!(!args.tty);
        assert!(!args.stdin);
        assert!(!args.detached);
        assert!(
            !args.auto_remove,
            "the executor takes the step's state itself, after it has read the upper",
        );
    }

    #[test]
    fn the_document_s_directory_stays_so_a_relative_fileset_or_mixin_still_roots() {
        assert_eq!(
            stepped().definition_dir,
            Some("/home/dev/agent".to_string())
        );
    }

    #[test]
    fn a_document_with_no_spec_is_refused_by_name() {
        let err = args_for(&base(), r#"{"kind":"sandbox"}"#, &step()).unwrap_err();
        assert!(format!("{err:#}").contains("declares no spec"), "{err:#}");
    }

    #[test]
    fn a_definition_that_is_not_json_is_refused_by_name() {
        let err = args_for(&base(), "not json", &step()).unwrap_err();
        assert!(
            format!("{err:#}").contains("reading the document a build step boots from"),
            "{err:#}"
        );
    }

    #[test]
    fn the_summary_line_names_the_file_the_architecture_and_the_digest() {
        let line = built_line(
            "./image/Dockerfile",
            "lns-build.local/built@sha256:abcdef0123456789",
            false,
        );
        assert!(
            line.starts_with("built from ./image/Dockerfile, "),
            "{line}"
        );
        assert!(line.contains("sha256:abcdef0123456789"), "{line}");
        assert!(
            line.contains(&crate::image::want_arch().to_string()),
            "an approver has to see which architecture was built: {line}",
        );
    }

    #[test]
    fn a_reference_with_no_digest_is_named_whole() {
        assert!(built_line("./Dockerfile", "alpine:3.20", false).ends_with("alpine:3.20"));
    }

    /// An approver reading a run summary has to be told the document's own rules decided nothing about this build.
    #[test]
    fn an_image_the_host_daemon_built_says_so_in_the_line_the_run_summary_prints() {
        let line = built_line("./image/Containerfile", "built@sha256:aa", true);
        assert!(
            line.ends_with("— built outside the gate by the host Docker daemon"),
            "{line}"
        );
    }
}
