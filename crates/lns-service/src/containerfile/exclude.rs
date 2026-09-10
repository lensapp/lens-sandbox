use super::upper::{Change, ChangeSet};

/// What lns itself writes into the rootfs before the workload runs: this host's DNS, its own trust
/// store, the identity files a run-as user needs, and the state directory the guest keeps.
const BOOT_WRITTEN: &[&str] = &[
    ".lens",
    "etc/resolv.conf",
    "etc/hosts",
    "etc/ssl",
    "etc/passwd",
    "etc/group",
];

/// Where PID 1 records the paths it created for the guest, in-root and absolute, one per line.
const BOOT_WRITTEN_RECORD: &str = ".lens/boot-written";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dropped {
    pub boot: usize,
    pub fileset: usize,
}

impl Dropped {
    pub(crate) fn total(&self) -> usize {
        self.boot + self.fileset
    }
}

/// Everything the workload wrote, without what this boot wrote for it: a layer outlives the run, and
/// a fileset's content is a secret the document keeps outside the workload's image.
pub(crate) fn only_the_workloads_writes(
    changes: ChangeSet,
    fileset_paths: &[String],
) -> (ChangeSet, Dropped) {
    let seeded: Vec<&str> = fileset_paths
        .iter()
        .map(|path| path.trim_start_matches('/').trim_end_matches('/'))
        .filter(|path| !path.is_empty())
        .collect();
    let recorded = boot_written_record(&changes);
    let mut dropped = Dropped::default();
    let mut kept = Vec::with_capacity(changes.changes.len());
    for change in changes.changes {
        if covered_by(change.path(), BOOT_WRITTEN.iter().copied())
            || recorded.iter().any(|made| made == change.path())
        {
            dropped.boot += 1;
        } else if covered_by(change.path(), seeded.iter().copied()) {
            dropped.fileset += 1;
        } else {
            kept.push(change);
        }
    }
    (ChangeSet { changes: kept }, dropped)
}

/// The record names the paths PID 1 created and nothing under them: `/home/<user>` is the boot's, and what the workload then wrote inside it is the workload's to commit.
fn boot_written_record(changes: &ChangeSet) -> Vec<String> {
    changes
        .changes
        .iter()
        .find_map(|change| match change {
            Change::Regular { path, bytes, .. } if path == BOOT_WRITTEN_RECORD => Some(bytes),
            _ => None,
        })
        .map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::trim)
                .filter(|path| path.starts_with('/'))
                .map(|path| {
                    path.trim_start_matches('/')
                        .trim_end_matches('/')
                        .to_string()
                })
                .filter(|path| !path.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn covered_by<'a>(path: &str, mut excluded: impl Iterator<Item = &'a str>) -> bool {
    excluded.any(|entry| {
        path == entry
            || path
                .strip_prefix(entry)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containerfile::upper::Change;

    fn regular(path: &str) -> Change {
        Change::Regular {
            path: path.into(),
            mode: 0o600,
            uid: 0,
            gid: 0,
            bytes: b"secret".to_vec(),
        }
    }

    fn record(listing: &str) -> Change {
        Change::Regular {
            path: ".lens/boot-written".into(),
            mode: 0o600,
            uid: 0,
            gid: 0,
            bytes: listing.as_bytes().to_vec(),
        }
    }

    fn kept(changes: Vec<Change>, filesets: &[String]) -> Vec<String> {
        let (kept, _) = only_the_workloads_writes(ChangeSet { changes }, filesets);
        kept.changes.iter().map(|c| c.path().to_string()).collect()
    }

    /// The mahler-shaped document seeds a credential through a fileset; the run's layer is kept and
    /// listed as an image, so a seeded path in it would outlive the run it belongs to.
    #[test]
    fn a_fileset_seeded_credential_never_reaches_the_layer() {
        let seeded = vec!["/home/lns/.claude/.credentials.json".to_string()];

        let paths = kept(
            vec![
                regular("home/lns/.claude/.credentials.json"),
                regular("opt/build/output"),
            ],
            &seeded,
        );

        assert_eq!(paths, vec!["opt/build/output"]);
    }

    #[test]
    fn every_path_under_a_seeded_directory_goes_with_it() {
        let seeded = vec!["/opt/tools".to_string()];

        let paths = kept(
            vec![
                regular("opt/tools/bin/kubectl"),
                regular("opt/tools"),
                regular("opt/toolsmith/keep"),
            ],
            &seeded,
        );

        assert_eq!(paths, vec!["opt/toolsmith/keep"]);
    }

    #[test]
    fn what_the_boot_wrote_for_the_run_is_not_what_the_run_built() {
        let paths = kept(
            vec![
                regular(".lens/dhcp-dns"),
                regular("etc/resolv.conf"),
                regular("etc/hosts"),
                regular("etc/ssl/certs/ca-certificates.crt"),
                regular("etc/passwd"),
                regular("etc/group"),
                regular("etc/alpine-release"),
                regular("spike-created"),
            ],
            &[],
        );

        assert_eq!(paths, vec!["etc/alpine-release", "spike-created"]);
    }

    /// `USER node` then `RUN pip install --user` writes under the very home the boot made, and a
    /// whole-prefix exclusion would commit that step as an empty layer.
    #[test]
    fn the_home_the_boot_made_goes_and_what_the_workload_wrote_under_it_stays() {
        let paths = kept(
            vec![
                record("/home/node\n"),
                Change::Directory {
                    path: "home/node".into(),
                    mode: 0o755,
                    uid: 1000,
                    gid: 1000,
                },
                regular("home/node/.local/lib/python3.12/site-packages/httpx/__init__.py"),
            ],
            &[],
        );

        assert_eq!(
            paths,
            vec!["home/node/.local/lib/python3.12/site-packages/httpx/__init__.py"]
        );
    }

    #[test]
    fn a_mount_point_the_boot_made_anywhere_goes_by_the_record_it_left() {
        let paths = kept(
            vec![
                record("/srv/state\n\n  /var/cache/agent  \nnot-a-path\n/\n"),
                regular("srv/state"),
                regular("var/cache/agent"),
                regular("srv/state-of-the-art"),
            ],
            &[],
        );

        assert_eq!(paths, vec!["srv/state-of-the-art"]);
    }

    #[test]
    fn a_boot_that_recorded_nothing_drops_nothing_of_the_workload_s() {
        let paths = kept(vec![regular("home/node/.bashrc")], &[]);

        assert_eq!(paths, vec!["home/node/.bashrc"]);
    }

    #[test]
    fn a_path_that_only_starts_with_an_excluded_name_is_kept() {
        let paths = kept(
            vec![
                regular("etc/hostsfile"),
                regular("homework/notes"),
                regular(".lensrc"),
            ],
            &[],
        );

        assert_eq!(
            paths,
            vec!["etc/hostsfile", "homework/notes", ".lensrc"],
            "the prefix must match whole path segments",
        );
    }

    /// A deletion of a path lns seeded is lns's own business too: whiting it out would hide a file the
    /// base image never shipped.
    #[test]
    fn a_deletion_of_an_excluded_path_is_dropped_with_it() {
        let paths = kept(
            vec![
                Change::Removed {
                    path: "etc/resolv.conf".into(),
                },
                Change::Removed {
                    path: "etc/alpine-release".into(),
                },
            ],
            &[],
        );

        assert_eq!(paths, vec!["etc/alpine-release"]);
    }

    #[test]
    fn the_count_says_how_many_went_and_which_rule_took_them() {
        let (_, dropped) = only_the_workloads_writes(
            ChangeSet {
                changes: vec![
                    regular("etc/resolv.conf"),
                    record("/home/lns\n"),
                    regular("home/lns"),
                    regular("opt/tools/bin/kubectl"),
                    regular("spike-created"),
                ],
            },
            &[
                "/opt/tools/bin/kubectl".to_string(),
                "/home/lns".to_string(),
            ],
        );

        assert_eq!(
            dropped,
            Dropped {
                boot: 3,
                fileset: 1
            },
            "a path both rules would take counts once, under the boot rule",
        );
        assert_eq!(dropped.total(), 4);
    }

    #[test]
    fn a_run_that_wrote_nothing_of_lns_drops_nothing() {
        let (kept, dropped) = only_the_workloads_writes(
            ChangeSet {
                changes: vec![regular("spike-created")],
            },
            &[],
        );

        assert_eq!(kept.changes.len(), 1);
        assert_eq!(dropped.total(), 0);
    }
}
