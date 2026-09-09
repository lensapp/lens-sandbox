#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::parse::parse;
    use crate::containerfile::upper::{Change, ChangeSet};
    use std::sync::Mutex;

    /// What the loop asked the host to do, in the order it asked.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Base(String),
        Run(RunStep),
        Copy(CopyStep),
        Commit {
            parent: String,
            layer: Option<ChangeSet>,
            config: ConfigDraft,
            created_by: String,
        },
    }

    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<Call>>,
        commits: Mutex<usize>,
        wrote: Mutex<Option<ChangeSet>>,
        fileset_paths: Vec<String>,
        run_fails_on_line: Option<usize>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self::default()
        }

        /// What every RUN's guest leaves in its upper, so one build can be read as one change set.
        fn writing(mut self, changes: ChangeSet) -> Self {
            self.wrote = Mutex::new(Some(changes));
            self
        }

        fn seeding(mut self, paths: &[&str]) -> Self {
            self.fileset_paths = paths.iter().map(|p| p.to_string()).collect();
            self
        }

        fn failing_on_line(mut self, line: usize) -> Self {
            self.run_fails_on_line = Some(line);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn commits(&self) -> Vec<(Option<ChangeSet>, ConfigDraft, String)> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Commit {
                        layer,
                        config,
                        created_by,
                        ..
                    } => Some((layer, config, created_by)),
                    _ => None,
                })
                .collect()
        }

        /// The config the finished image carries, which is the last one committed.
        fn final_config(&self) -> ConfigDraft {
            self.commits()
                .pop()
                .expect("a build commits at least once")
                .1
        }

        fn runs(&self) -> Vec<RunStep> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Run(step) => Some(step),
                    _ => None,
                })
                .collect()
        }
    }

    impl BuildHost for FakeHost {
        async fn resolve_base(&self, image: &str) -> Result<String> {
            self.calls.lock().unwrap().push(Call::Base(image.into()));
            Ok(format!("registry.test/{image}@sha256:base"))
        }

        async fn run(&self, step: &RunStep) -> Result<RunOutcome> {
            self.calls.lock().unwrap().push(Call::Run(step.clone()));
            if self.run_fails_on_line == Some(step.line) {
                anyhow::bail!("the build guest's command exited 2");
            }
            Ok(RunOutcome {
                changes: self.wrote.lock().unwrap().clone().unwrap_or_default(),
                fileset_paths: self.fileset_paths.clone(),
            })
        }

        async fn copy(&self, step: &CopyStep) -> Result<ChangeSet> {
            self.calls.lock().unwrap().push(Call::Copy(step.clone()));
            Ok(ChangeSet {
                changes: vec![Change::Regular {
                    path: step.destination.trim_start_matches('/').into(),
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                    bytes: b"context\n".to_vec(),
                }],
            })
        }

        async fn commit(&self, commit: &Commit<'_>) -> Result<String> {
            let mut committed = self.commits.lock().unwrap();
            *committed += 1;
            self.calls.lock().unwrap().push(Call::Commit {
                parent: commit.parent.into(),
                layer: commit.layer.cloned(),
                config: commit.config.clone(),
                created_by: commit.created_by.into(),
            });
            Ok(format!("lns-build.local/built@sha256:step{committed}"))
        }
    }

    fn containerfile(text: &str) -> crate::containerfile::parse::Containerfile {
        parse(text).expect("the subset accepts this file")
    }

    async fn built(host: &FakeHost, text: &str) -> Built {
        build(host, &containerfile(text))
            .await
            .expect("this Containerfile builds")
    }

    async fn refused(host: &FakeHost, text: &str) -> String {
        format!(
            "{:#}",
            build(host, &containerfile(text))
                .await
                .expect_err("this Containerfile must stop the build")
        )
    }

    #[tokio::test]
    async fn the_instructions_run_in_the_order_they_were_written_and_each_stands_on_the_last() {
        let host = FakeHost::new();
        let built = built(
            &host,
            "FROM alpine:3.20\n\
             RUN echo one\n\
             COPY app /srv/app\n\
             RUN echo two\n",
        )
        .await;

        let order: Vec<String> = host
            .calls()
            .into_iter()
            .map(|call| match call {
                Call::Base(image) => format!("base {image}"),
                Call::Run(step) => format!("run {}", step.argv.join(" ")),
                Call::Copy(step) => format!("copy {}", step.destination),
                Call::Commit { parent, .. } => format!("commit over {parent}"),
            })
            .collect();

        assert_eq!(
            order,
            vec![
                "base alpine:3.20".to_string(),
                "run /bin/sh -c echo one".to_string(),
                "commit over registry.test/alpine:3.20@sha256:base".to_string(),
                "copy /srv/app".to_string(),
                "commit over lns-build.local/built@sha256:step1".to_string(),
                "run /bin/sh -c echo two".to_string(),
                "commit over lns-build.local/built@sha256:step2".to_string(),
            ],
        );
        assert_eq!(built.reference, "lns-build.local/built@sha256:step3");
        assert_eq!(built.layers, 3);
    }

    #[tokio::test]
    async fn a_run_runs_in_a_guest_booted_from_the_image_the_build_has_so_far() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN echo one\nRUN echo two\n").await;

        let parents: Vec<String> = host.runs().into_iter().map(|step| step.parent).collect();
        assert_eq!(
            parents,
            vec![
                "registry.test/alpine@sha256:base".to_string(),
                "lns-build.local/built@sha256:step1".to_string(),
            ],
            "the next RUN boots the image the last instruction produced",
        );
    }

    #[tokio::test]
    async fn only_the_filesystem_instructions_produce_a_layer_and_the_rest_write_config() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\n\
             ENV MODE=research\n\
             RUN echo one\n\
             LABEL org.opencontainers.image.title=agent\n\
             COPY app /srv/app\n\
             USER node\n\
             WORKDIR /srv\n\
             ADD extra /srv/extra\n\
             ENTRYPOINT [\"/bin/agent\"]\n\
             CMD [\"--serve\"]\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             EXPOSE 8080\n\
             VOLUME /data\n",
        )
        .await;

        let produced: Vec<(bool, String)> = host
            .commits()
            .into_iter()
            .map(|(layer, _, created_by)| (layer.is_some(), created_by))
            .collect();

        assert_eq!(
            produced,
            vec![
                (false, "ENV MODE=research".to_string()),
                (true, "RUN echo one".to_string()),
                (
                    false,
                    "LABEL org.opencontainers.image.title=agent".to_string()
                ),
                (true, "COPY app /srv/app".to_string()),
                (false, "USER node".to_string()),
                (false, "WORKDIR /srv".to_string()),
                (true, "ADD extra /srv/extra".to_string()),
                (false, "ENTRYPOINT [\"/bin/agent\"]".to_string()),
                (false, "CMD [\"--serve\"]".to_string()),
                (false, "SHELL [\"/bin/bash\",\"-c\"]".to_string()),
                (false, "EXPOSE 8080".to_string()),
                (false, "VOLUME /data".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn every_config_instruction_writes_the_field_it_owns() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\n\
             ENV MODE=research HOME=/home/node\n\
             LABEL org.opencontainers.image.title=agent\n\
             USER node\n\
             WORKDIR /srv\n\
             ENTRYPOINT [\"/bin/agent\"]\n\
             CMD [\"--serve\"]\n\
             SHELL [\"/bin/bash\", \"-c\"]\n\
             EXPOSE 8080\n\
             VOLUME /data\n",
        )
        .await;

        assert_eq!(
            host.final_config(),
            ConfigDraft {
                env: vec![
                    ("MODE".into(), "research".into()),
                    ("HOME".into(), "/home/node".into()),
                ],
                labels: vec![("org.opencontainers.image.title".into(), "agent".into())],
                user: Some("node".into()),
                workdir: Some("/srv".into()),
                entrypoint: Some(vec!["/bin/agent".into()]),
                cmd: Some(vec!["--serve".into()]),
                shell: Some(vec!["/bin/bash".into(), "-c".into()]),
                exposed_ports: vec!["8080".into()],
                volumes: vec!["/data".into()],
            },
        );
    }

    #[tokio::test]
    async fn a_later_env_replaces_the_value_the_earlier_one_set() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nENV MODE=first\nENV MODE=second\n").await;

        assert_eq!(
            host.final_config().env,
            vec![("MODE".to_string(), "second".to_string())],
            "one key holds one value, and the last instruction decides it",
        );
    }

    #[tokio::test]
    async fn an_arg_reaches_the_next_run_and_never_the_image_config() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG CLAUDE_CODE_VERSION=2.1.263\nRUN npm i -g claude@$CLAUDE_CODE_VERSION\n",
        )
        .await;

        assert_eq!(
            host.runs()[0].env,
            vec!["CLAUDE_CODE_VERSION=2.1.263".to_string()],
            "a build argument is what the RUN's shell expands",
        );
        assert!(
            host.final_config().env.is_empty(),
            "an ARG is a build-time value, so the built image must not carry it",
        );
    }

    #[tokio::test]
    async fn an_env_reaches_the_next_run_as_well_as_the_image_config() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nENV MODE=research\nRUN echo $MODE\n").await;

        assert_eq!(host.runs()[0].env, vec!["MODE=research".to_string()]);
        assert_eq!(
            host.final_config().env,
            vec![("MODE".to_string(), "research".to_string())],
        );
    }

    #[tokio::test]
    async fn an_env_outranks_an_arg_of_the_same_name_the_way_docker_defines_it() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG MODE=from-arg\nENV MODE=from-env\nRUN echo $MODE\n",
        )
        .await;

        assert_eq!(host.runs()[0].env, vec!["MODE=from-env".to_string()]);
    }

    #[tokio::test]
    async fn an_arg_declared_before_from_belongs_to_no_stage_until_it_is_declared_again() {
        let host = FakeHost::new();
        built(
            &host,
            "ARG BASE_TAG=3.20\nFROM alpine:$BASE_TAG\nRUN echo $BASE_TAG\n",
        )
        .await;

        assert_eq!(
            host.calls()[0],
            Call::Base("alpine:3.20".into()),
            "an ARG before FROM is what the base reference expands with",
        );
        assert!(
            host.runs()[0].env.is_empty(),
            "Docker keeps a global ARG out of the stage until the stage declares it again",
        );
    }

    #[tokio::test]
    async fn a_stage_that_declares_a_global_arg_again_inherits_its_value() {
        let host = FakeHost::new();
        built(
            &host,
            "ARG BASE_TAG=3.20\nFROM alpine:$BASE_TAG\nARG BASE_TAG\nRUN echo $BASE_TAG\n",
        )
        .await;

        assert_eq!(host.runs()[0].env, vec!["BASE_TAG=3.20".to_string()]);
    }

    #[tokio::test]
    async fn an_arg_with_no_value_anywhere_reaches_the_run_as_nothing_at_all() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nARG UNSET\nRUN echo $UNSET\n").await;

        assert!(host.runs()[0].env.is_empty());
    }

    #[tokio::test]
    async fn a_user_and_a_workdir_apply_to_every_run_after_them() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN echo first\nUSER node\nWORKDIR /srv\nRUN echo second\n",
        )
        .await;

        let steps = host.runs();
        assert_eq!((steps[0].user.as_str(), steps[0].workdir.as_str()), ("root", "/"));
        assert_eq!(
            (steps[1].user.as_str(), steps[1].workdir.as_str()),
            ("node", "/srv"),
            "a RUN takes the identity and the directory the instructions before it set",
        );
    }

    #[tokio::test]
    async fn a_relative_workdir_joins_the_one_in_force() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nWORKDIR /srv\nWORKDIR app\nRUN echo hi\n",
        )
        .await;

        assert_eq!(host.runs()[0].workdir, "/srv/app");
        assert_eq!(host.final_config().workdir, Some("/srv/app".to_string()));
    }

    #[tokio::test]
    async fn a_shell_instruction_decides_how_the_next_run_in_shell_form_is_run() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nRUN echo first\nSHELL [\"/bin/bash\", \"-lc\"]\nRUN echo second\n",
        )
        .await;

        let argv: Vec<Vec<String>> = host.runs().into_iter().map(|step| step.argv).collect();
        assert_eq!(
            argv,
            vec![
                vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo first".to_string()
                ],
                vec![
                    "/bin/bash".to_string(),
                    "-lc".to_string(),
                    "echo second".to_string()
                ],
            ],
        );
    }

    #[tokio::test]
    async fn a_run_in_exec_form_is_run_without_any_shell() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN [\"/bin/agent\", \"--check\"]\n").await;

        assert_eq!(
            host.runs()[0].argv,
            vec!["/bin/agent".to_string(), "--check".to_string()],
        );
    }

    #[tokio::test]
    async fn a_here_document_run_is_run_as_the_script_it_holds() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nRUN <<EOF\necho one\necho two\nEOF\n").await;

        assert_eq!(
            host.runs()[0].argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo one\necho two\n".to_string()
            ],
        );
    }

    #[tokio::test]
    async fn what_this_boot_wrote_for_the_run_is_kept_out_of_every_captured_layer() {
        let host = FakeHost::new()
            .writing(ChangeSet {
                changes: vec![
                    change("opt/tool/bin"),
                    change(".lens/.cmdline"),
                    change("etc/resolv.conf"),
                    change("opt/agent-skills/prompts.md"),
                ],
            })
            .seeding(&["/opt/agent-skills"]);

        built(&host, "FROM alpine\nRUN install-a-tool\nRUN install-another\n").await;

        for (layer, _, created_by) in host.commits() {
            let paths: Vec<String> = layer
                .expect("a RUN commits a layer")
                .changes
                .iter()
                .map(|change| change.path().to_string())
                .collect();
            assert_eq!(
                paths,
                vec!["opt/tool/bin".to_string()],
                "{created_by} carried what lns wrote for the guest, or what a fileset seeded",
            );
        }
    }

    fn change(path: &str) -> Change {
        Change::Regular {
            path: path.into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            bytes: b"x".to_vec(),
        }
    }

    #[tokio::test]
    async fn a_copy_lands_where_the_workdir_in_force_puts_it() {
        let host = FakeHost::new();
        built(&host, "FROM alpine\nWORKDIR /srv\nCOPY app dist\n").await;

        let Call::Copy(step) = host
            .calls()
            .into_iter()
            .find(|call| matches!(call, Call::Copy(_)))
            .expect("the COPY reached the host")
        else {
            unreachable!()
        };
        assert_eq!(step.destination, "/srv/dist");
        assert_eq!(step.sources, vec!["app".to_string()]);
    }

    #[tokio::test]
    async fn a_copy_expands_a_build_argument_in_its_paths() {
        let host = FakeHost::new();
        built(
            &host,
            "FROM alpine\nARG VERSION=1.2.3\nCOPY dist/$VERSION /srv/app\n",
        )
        .await;

        let Call::Copy(step) = host
            .calls()
            .into_iter()
            .find(|call| matches!(call, Call::Copy(_)))
            .expect("the COPY reached the host")
        else {
            unreachable!()
        };
        assert_eq!(step.sources, vec!["dist/1.2.3".to_string()]);
    }

    #[tokio::test]
    async fn a_run_that_fails_stops_the_build_naming_its_line_and_what_it_was() {
        let host = FakeHost::new().failing_on_line(3);
        let refusal = refused(
            &host,
            "FROM alpine\nRUN echo one\nRUN reach-an-undeclared-host\n",
        )
        .await;

        assert!(refusal.contains("line 3"), "{refusal}");
        assert!(refusal.contains("RUN reach-an-undeclared-host"), "{refusal}");
        assert!(refusal.contains("exited 2"), "{refusal}");
        assert_eq!(
            host.runs().len(),
            2,
            "the instruction after the failure must not run",
        );
    }

    #[tokio::test]
    async fn a_file_that_does_not_begin_with_from_stops_the_build_naming_the_line() {
        let host = FakeHost::new();
        let refusal = refused(&host, "RUN echo hi\nFROM alpine\n").await;

        assert!(refusal.contains("FROM"), "{refusal}");
        assert!(refusal.contains("line 1"), "{refusal}");
        assert!(
            host.calls().is_empty(),
            "a build that cannot start must reach no guest and no store",
        );
    }

    #[tokio::test]
    async fn a_file_with_no_instruction_at_all_stops_the_build() {
        let host = FakeHost::new();
        let refusal = refused(&host, "# nothing but a comment\n").await;

        assert!(refusal.contains("FROM"), "{refusal}");
    }

    #[tokio::test]
    async fn a_containerfile_that_only_names_its_base_boots_that_base() {
        let host = FakeHost::new();
        let built = built(&host, "FROM alpine:3.20\n").await;

        assert_eq!(built.reference, "registry.test/alpine:3.20@sha256:base");
        assert_eq!(built.layers, 0);
        assert!(
            host.commits().is_empty(),
            "a file that changes nothing about the base has nothing to commit",
        );
    }

    #[tokio::test]
    async fn a_base_the_build_cannot_resolve_stops_it_before_any_instruction_runs() {
        struct NoBase;
        impl BuildHost for NoBase {
            async fn resolve_base(&self, image: &str) -> Result<String> {
                anyhow::bail!("no such image {image}")
            }
            async fn run(&self, _step: &RunStep) -> Result<RunOutcome> {
                unreachable!("the base is resolved first")
            }
            async fn copy(&self, _step: &CopyStep) -> Result<ChangeSet> {
                unreachable!("the base is resolved first")
            }
            async fn commit(&self, _commit: &Commit<'_>) -> Result<String> {
                unreachable!("the base is resolved first")
            }
        }

        let refusal = format!(
            "{:#}",
            build(&NoBase, &containerfile("FROM missing:1\nRUN echo hi\n"))
                .await
                .unwrap_err()
        );
        assert!(refusal.contains("line 1"), "{refusal}");
        assert!(refusal.contains("FROM missing:1"), "{refusal}");
    }

    #[tokio::test]
    async fn a_copy_the_context_cannot_answer_stops_the_build_naming_its_line() {
        struct NoContext;
        impl BuildHost for NoContext {
            async fn resolve_base(&self, image: &str) -> Result<String> {
                Ok(image.to_string())
            }
            async fn run(&self, _step: &RunStep) -> Result<RunOutcome> {
                unreachable!("this file has no RUN")
            }
            async fn copy(&self, _step: &CopyStep) -> Result<ChangeSet> {
                anyhow::bail!("no such file in the build context")
            }
            async fn commit(&self, _commit: &Commit<'_>) -> Result<String> {
                unreachable!("nothing is committed")
            }
        }

        let refusal = format!(
            "{:#}",
            build(&NoContext, &containerfile("FROM alpine\nCOPY missing /srv\n"))
                .await
                .unwrap_err()
        );
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("COPY missing /srv"), "{refusal}");
        assert!(refusal.contains("build context"), "{refusal}");
    }

    #[tokio::test]
    async fn a_commit_the_store_refuses_stops_the_build_naming_the_instruction() {
        struct NoStore;
        impl BuildHost for NoStore {
            async fn resolve_base(&self, image: &str) -> Result<String> {
                Ok(image.to_string())
            }
            async fn run(&self, _step: &RunStep) -> Result<RunOutcome> {
                unreachable!("this file has no RUN")
            }
            async fn copy(&self, _step: &CopyStep) -> Result<ChangeSet> {
                unreachable!("this file has no COPY")
            }
            async fn commit(&self, _commit: &Commit<'_>) -> Result<String> {
                anyhow::bail!("the layer cache is not writable")
            }
        }

        let refusal = format!(
            "{:#}",
            build(&NoStore, &containerfile("FROM alpine\nENV MODE=research\n"))
                .await
                .unwrap_err()
        );
        assert!(refusal.contains("line 2"), "{refusal}");
        assert!(refusal.contains("ENV MODE=research"), "{refusal}");
    }
}
