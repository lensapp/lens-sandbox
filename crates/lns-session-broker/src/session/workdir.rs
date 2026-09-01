use std::io;

pub(crate) trait WorkdirFs {
    fn is_dir(&self, path: &str) -> bool;
    fn create_dir(&self, path: &str) -> io::Result<()>;
    fn chown(&self, path: &str, uid: u32, gid: u32) -> io::Result<()>;
    fn set_current_dir(&self, path: &str) -> io::Result<()>;
}

const ROOT_UID: u32 = 0;

/// A workdir the guest creates on the workload's behalf belongs to the run-as identity, the way a container runtime creates a missing `WORKDIR`; only the directories this call creates are chowned, so an image's own ownership is never rewritten.
pub(crate) fn enter(
    fs: &dyn WorkdirFs,
    dir: &str,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    for ancestor in ancestors(dir) {
        if fs.is_dir(&ancestor) {
            continue;
        }
        fs.create_dir(&ancestor)
            .map_err(|e| format!("creating {ancestor:?}: {e}"))?;
        if let Some((uid, gid)) = owner.filter(|(uid, _)| *uid != ROOT_UID) {
            fs.chown(&ancestor, uid, gid)
                .map_err(|e| format!("chown({ancestor:?}, {uid}:{gid}): {e}"))?;
        }
    }
    fs.set_current_dir(dir)
        .map_err(|e| format!("entering {dir:?}: {e}"))
}

fn ancestors(dir: &str) -> Vec<String> {
    let absolute = dir.starts_with('/');
    let mut acc = String::new();
    let mut out = Vec::new();
    for component in dir.split('/').filter(|c| !c.is_empty()) {
        if absolute || !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(component);
        out.push(acc.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Create(String),
        Chown(String, u32, u32),
        Chdir(String),
    }

    struct FakeFs {
        existing: Vec<String>,
        calls: RefCell<Vec<Call>>,
        create_fails: Option<String>,
        chown_fails: bool,
        chdir_fails: bool,
    }

    impl FakeFs {
        fn with_existing(existing: &[&str]) -> Self {
            Self {
                existing: existing.iter().map(|p| (*p).to_string()).collect(),
                calls: RefCell::new(Vec::new()),
                create_fails: None,
                chown_fails: false,
                chdir_fails: false,
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.replace(Vec::new())
        }
    }

    fn denied() -> io::Error {
        io::Error::from(io::ErrorKind::PermissionDenied)
    }

    impl WorkdirFs for FakeFs {
        fn is_dir(&self, path: &str) -> bool {
            self.existing.iter().any(|p| p == path)
        }

        fn create_dir(&self, path: &str) -> io::Result<()> {
            if self.create_fails.as_deref() == Some(path) {
                return Err(denied());
            }
            self.calls.borrow_mut().push(Call::Create(path.to_string()));
            Ok(())
        }

        fn chown(&self, path: &str, uid: u32, gid: u32) -> io::Result<()> {
            if self.chown_fails {
                return Err(denied());
            }
            self.calls
                .borrow_mut()
                .push(Call::Chown(path.to_string(), uid, gid));
            Ok(())
        }

        fn set_current_dir(&self, path: &str) -> io::Result<()> {
            if self.chdir_fails {
                return Err(denied());
            }
            self.calls.borrow_mut().push(Call::Chdir(path.to_string()));
            Ok(())
        }
    }

    #[test]
    fn a_workdir_the_guest_creates_belongs_to_the_run_as_user() {
        let fs = FakeFs::with_existing(&["/", "/home", "/home/node"]);
        enter(&fs, "/home/node/task", Some((1000, 1000))).expect("the workdir is created");
        assert_eq!(
            fs.calls(),
            vec![
                Call::Create("/home/node/task".into()),
                Call::Chown("/home/node/task".into(), 1000, 1000),
                Call::Chdir("/home/node/task".into()),
            ],
            "a pre-start script running as the run-as user must be able to write into the run's own declared workdir"
        );
    }

    #[test]
    fn every_parent_the_guest_has_to_create_is_owned_by_the_run_as_user_too() {
        let fs = FakeFs::with_existing(&["/", "/srv"]);
        enter(&fs, "/srv/a/b", Some((1000, 33))).expect("both levels are created");
        assert_eq!(
            fs.calls(),
            vec![
                Call::Create("/srv/a".into()),
                Call::Chown("/srv/a".into(), 1000, 33),
                Call::Create("/srv/a/b".into()),
                Call::Chown("/srv/a/b".into(), 1000, 33),
                Call::Chdir("/srv/a/b".into()),
            ],
            "a leaf the run-as user owns is unreachable if the parent it hangs from stays root-only"
        );
    }

    #[test]
    fn a_workdir_the_image_already_ships_keeps_its_own_ownership() {
        let fs = FakeFs::with_existing(&["/", "/app"]);
        enter(&fs, "/app", Some((1000, 1000))).expect("nothing to create");
        assert_eq!(
            fs.calls(),
            vec![Call::Chdir("/app".into())],
            "the guest only owns what it creates; rewriting an image's ownership is not its business"
        );
    }

    #[test]
    fn a_root_run_as_needs_no_chown() {
        let fs = FakeFs::with_existing(&["/"]);
        enter(&fs, "/work", Some((0, 0))).expect("created");
        assert_eq!(
            fs.calls(),
            vec![Call::Create("/work".into()), Call::Chdir("/work".into())],
            "root already writes any path, so the chown would be a no-op syscall that can only fail"
        );
    }

    #[test]
    fn an_unresolved_run_as_identity_leaves_the_new_workdir_as_created() {
        let fs = FakeFs::with_existing(&["/"]);
        enter(&fs, "/work", None).expect("created");
        assert_eq!(
            fs.calls(),
            vec![Call::Create("/work".into()), Call::Chdir("/work".into())],
            "lns-init could not resolve ids, so there is no identity to hand the directory to"
        );
    }

    #[test]
    fn a_relative_workdir_is_created_component_by_component() {
        let fs = FakeFs::with_existing(&[]);
        enter(&fs, "a/b", Some((7, 7))).expect("created");
        assert_eq!(
            fs.calls(),
            vec![
                Call::Create("a".into()),
                Call::Chown("a".into(), 7, 7),
                Call::Create("a/b".into()),
                Call::Chown("a/b".into(), 7, 7),
                Call::Chdir("a/b".into()),
            ],
            "a relative workdir must not be turned into an absolute one"
        );
    }

    #[test]
    fn a_creation_failure_names_the_path_that_could_not_be_made() {
        let mut fs = FakeFs::with_existing(&["/"]);
        fs.create_fails = Some("/work".into());
        let err = enter(&fs, "/work/inner", Some((1000, 1000))).expect_err("creation fails");
        assert!(
            err.contains("creating \"/work\""),
            "the message must name the failing path: {err}"
        );
        assert_eq!(fs.calls(), Vec::new(), "the walk stops at the failure");
    }

    #[test]
    fn a_failed_chown_fails_the_session_rather_than_handing_over_a_root_owned_workdir() {
        let mut fs = FakeFs::with_existing(&["/"]);
        fs.chown_fails = true;
        let err = enter(&fs, "/work", Some((1000, 1000))).expect_err("the chown fails");
        assert!(
            err.contains("chown(\"/work\", 1000:1000)"),
            "a silently root-owned workdir is the bug this call exists to prevent: {err}"
        );
    }

    #[test]
    fn a_workdir_that_cannot_be_entered_is_reported() {
        let mut fs = FakeFs::with_existing(&["/", "/app"]);
        fs.chdir_fails = true;
        let err = enter(&fs, "/app", None).expect_err("chdir fails");
        assert!(err.contains("entering \"/app\""), "{err}");
    }
}
