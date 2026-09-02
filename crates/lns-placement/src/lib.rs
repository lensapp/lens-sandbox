//! Where a written file lands against one mount, per `docs/sandbox-spec.md` §3.1.11.

/// Where the run leaves the guest the paths its installed connectors would write, so both sides name one file (§3.1.11).
pub const CONNECTOR_WRITES_MANIFEST: &str = "/.lens/connector-claims";

/// What a mount does to a path something writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The mount is elsewhere: the write lands where it was aimed.
    Free,
    /// An `exclude` of this bind leaves the path guest-local, and this is the entry that does it.
    Masked { exclude: usize },
    /// The mount covers the path, so the write would reach the share behind it.
    WritesThrough,
}

/// A path split into the segments a claim is decided on, so a trailing or doubled separator is not a second path.
pub fn claimed_path(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect()
}

/// Two spellings of one path.
pub fn same_path(left: &str, right: &str) -> bool {
    claimed_path(left) == claimed_path(right)
}

/// Whether `outer` is a strict path prefix of `inner`.
pub fn encloses(outer: &str, inner: &str) -> bool {
    let mut inner_segments = claimed_path(inner).into_iter();
    claimed_path(outer)
        .into_iter()
        .all(|segment| inner_segments.next() == Some(segment))
        && inner_segments.next().is_some()
}

fn covers(outer: &str, inner: &str) -> bool {
    same_path(outer, inner) || encloses(outer, inner)
}

/// Whether this one `exclude` of the bind leaves `claim` guest-local. Two excludes can each cover one claim, and each still names an entry the guest leaves unmounted.
pub fn mask_covers(target: &str, exclude: &str, claim: &str) -> bool {
    covers(
        &format!("{}/{}", target.trim_end_matches('/'), exclude),
        claim,
    )
}

/// A volume takes no `exclude`, so it decides every claim it covers as [`Placement::WritesThrough`].
pub fn place<S: AsRef<str>>(target: &str, excludes: &[S], claim: &str) -> Placement {
    if !covers(target, claim) && !encloses(claim, target) {
        return Placement::Free;
    }
    let masked = excludes
        .iter()
        .position(|entry| mask_covers(target, entry.as_ref(), claim));
    match masked {
        Some(exclude) => Placement::Masked { exclude },
        None => Placement::WritesThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[&str] = &[];

    #[test]
    fn a_claim_beside_the_target_is_free_of_it() {
        assert_eq!(
            place("/root/.claude", NONE, "/root/.claude.json"),
            Placement::Free,
            "a sibling name that merely starts the same is not under the mount"
        );
    }

    #[test]
    fn a_claim_under_the_target_writes_through_when_no_exclude_covers_it() {
        assert_eq!(
            place(
                "/root/.claude",
                &["projects"],
                "/root/.claude/settings.json"
            ),
            Placement::WritesThrough
        );
    }

    #[test]
    fn a_target_under_the_claim_writes_through_too_because_the_mount_still_covers_part_of_it() {
        assert_eq!(
            place("/work/inner", NONE, "/work"),
            Placement::WritesThrough,
            "a claim on a whole directory reaches the mount nested inside it"
        );
    }

    #[test]
    fn a_claim_exactly_on_the_target_writes_through() {
        assert_eq!(
            place("/work", NONE, "/work"),
            Placement::WritesThrough,
            "landing on the mount point is landing under the mount"
        );
    }

    #[test]
    fn an_exclude_naming_the_claim_itself_masks_it() {
        assert_eq!(
            place(
                "/root/.claude",
                &[".credentials.json"],
                "/root/.claude/.credentials.json"
            ),
            Placement::Masked { exclude: 0 }
        );
    }

    #[test]
    fn an_exclude_naming_a_directory_masks_what_is_under_it() {
        assert_eq!(
            place(
                "/root/.claude",
                &["projects", "agent"],
                "/root/.claude/agent/state.json"
            ),
            Placement::Masked { exclude: 1 },
            "the answer names which entry does it, so the guest can leave that one unmounted"
        );
    }

    #[test]
    fn an_exclude_below_the_claim_does_not_mask_it() {
        assert_eq!(
            place("/work", &["cfg/inner"], "/work/cfg"),
            Placement::WritesThrough,
            "masking part of a claimed directory leaves the rest of it on the share"
        );
    }

    #[test]
    fn two_excludes_can_each_cover_one_claim_and_each_names_an_entry_to_leave_unmounted() {
        assert!(mask_covers(
            "/root/.claude",
            "sub/inner",
            "/root/.claude/sub/inner/x"
        ));
        assert!(
            mask_covers("/root/.claude", "sub", "/root/.claude/sub/inner/x"),
            "the outer entry is the one the guest actually leaves unmounted"
        );
        assert_eq!(
            place(
                "/root/.claude",
                &["sub/inner", "sub"],
                "/root/.claude/sub/inner/x"
            ),
            Placement::Masked { exclude: 0 },
            "one verdict names the first cover; a caller wanting every cover asks mask_covers"
        );
    }

    #[test]
    fn a_trailing_separator_is_the_same_path_and_not_a_second_one() {
        assert_eq!(
            place("/root/.claude/", &["agent/"], "/root/.claude/agent"),
            Placement::Masked { exclude: 0 }
        );
    }

    #[test]
    fn a_segment_that_merely_starts_the_same_is_a_different_path() {
        assert!(!encloses("/home/node", "/home/nodejs/.config"));
        assert!(!same_path("/home/node", "/home/nodejs"));
        assert!(same_path("/home/node", "/home/node/"));
        assert!(encloses("/home/node", "/home/node/.config"));
    }

    #[test]
    fn claimed_path_drops_the_separators_and_dots_that_spell_one_path_many_ways() {
        assert_eq!(
            claimed_path("//root/./.claude//agent/"),
            ["root", ".claude", "agent"]
        );
    }
}
