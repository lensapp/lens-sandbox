use anyhow::Result;

use crate::upperfs::constants::*;
use crate::upperfs::format::Superblock;
use crate::upperfs::layout::Layout;

/// What a grow must write, decided before anything is touched.
#[derive(Debug, Clone)]
pub struct GrowPlan {
    pub before: Layout,
    pub after: Layout,
    pub superblock: Superblock,
    /// Groups that did not exist before, in ascending order.
    pub appended_groups: std::ops::Range<u32>,
    /// The group whose short tail the grow fills in, and the blocks it gains.
    pub filled_tail: Option<(u32, std::ops::Range<u32>)>,
}

impl GrowPlan {
    pub fn image_size_bytes(&self) -> u64 {
        self.after.image_size_bytes()
    }
}

/// Decides what a grow towards `new_size_bytes` must change; `None` means nothing is written, because the image already meets the size or the only blocks it could add cannot hold a group.
pub fn plan_grow(superblock: &Superblock, new_size_bytes: u64) -> Result<Option<GrowPlan>> {
    let before = Layout::for_existing(
        superblock.blocks_count,
        superblock.inodes_per_group,
        superblock
            .blocks_count
            .div_ceil(DESCS_PER_BLOCK * BLOCKS_PER_GROUP),
        u32::from(superblock.reserved_gdt_blocks),
    );

    let requested_blocks = (new_size_bytes / BLOCK_SIZE as u64).min(u64::from(u32::MAX)) as u32;
    if requested_blocks <= before.block_count {
        return Ok(None);
    }

    // A volume that still owns a reserved run keeps one block of it: consuming the last one would leave the resize inode naming blocks the same grow just turned into descriptors.
    let keep = u32::from(before.reserved_gdt_blocks > 0);
    let largest_gdt = before.gdt_blocks + before.reserved_gdt_blocks - keep;
    let requested_gdt = requested_blocks
        .div_ceil(BLOCKS_PER_GROUP)
        .div_ceil(DESCS_PER_BLOCK);
    anyhow::ensure!(
        requested_gdt <= largest_gdt,
        "this volume can grow to at most {} bytes: its descriptor table would need {requested_gdt} blocks and it can reach {largest_gdt}",
        u64::from(largest_gdt * DESCS_PER_BLOCK)
            * u64::from(BLOCKS_PER_GROUP)
            * u64::from(BLOCK_SIZE)
    );

    let block_count = drop_a_tail_too_short_to_hold_itself(&before, requested_blocks);
    if block_count <= before.block_count {
        return Ok(None);
    }

    let num_groups = block_count.div_ceil(BLOCKS_PER_GROUP);
    let gdt_blocks = num_groups.div_ceil(DESCS_PER_BLOCK);

    let after = Layout::for_existing(
        block_count,
        before.inodes_per_group,
        gdt_blocks,
        reserved_after(&before, gdt_blocks),
    );

    let filled_tail = filled_tail(&before, &after);
    let mut superblock = superblock.clone();
    superblock.blocks_count = after.block_count;
    superblock.inodes_count = after.inodes_count;
    superblock.reserved_gdt_blocks = u16::try_from(after.reserved_gdt_blocks)
        .expect("the reserved run never exceeds one block of addresses");

    Ok(Some(GrowPlan {
        appended_groups: before.num_groups..after.num_groups,
        filled_tail,
        before,
        after,
        superblock,
    }))
}

/// A group too short to hold its own metadata is worse than no group at all, so the filesystem stops one group earlier and leaves the slack unused.
fn drop_a_tail_too_short_to_hold_itself(before: &Layout, requested_blocks: u32) -> u32 {
    let num_groups = requested_blocks.div_ceil(BLOCKS_PER_GROUP);
    let tail_blocks = requested_blocks - (num_groups - 1) * BLOCKS_PER_GROUP;
    if tail_blocks == BLOCKS_PER_GROUP {
        return requested_blocks;
    }
    let gdt_blocks = num_groups.div_ceil(DESCS_PER_BLOCK);
    let prospective = Layout::for_existing(
        requested_blocks,
        before.inodes_per_group,
        gdt_blocks,
        reserved_after(before, gdt_blocks),
    );
    let table = if prospective.group_has_backup(num_groups - 1) {
        1 + gdt_blocks + prospective.reserved_gdt_blocks
    } else {
        0
    };
    let own_metadata = table + 2 + before.inode_table_blocks;
    if tail_blocks > own_metadata {
        requested_blocks
    } else {
        (num_groups - 1) * BLOCKS_PER_GROUP
    }
}

fn reserved_after(before: &Layout, gdt_blocks: u32) -> u32 {
    before.gdt_blocks + before.reserved_gdt_blocks - gdt_blocks
}

/// The one existing group a grow edits: a short tail becomes longer, and the padding bits that stood for its missing blocks become free.
fn filled_tail(before: &Layout, after: &Layout) -> Option<(u32, std::ops::Range<u32>)> {
    let tail = before.num_groups - 1;
    let was = before.last_group_blocks;
    let now = after.blocks_in_group(tail);
    (now > was).then_some((tail, was..now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upperfs::plan::Plan;

    fn grown(from: u64, to: u64) -> Option<GrowPlan> {
        let plan = Plan::new(from, [0; 16], "test", 0).unwrap();
        plan_grow(&plan.superblock, to).unwrap()
    }

    #[test]
    fn a_grow_keeps_the_inode_density_the_image_was_created_with() {
        let g = grown(32 * 1024 * 1024, 10 * 1024 * 1024 * 1024).unwrap();
        assert_eq!(
            g.after.inodes_per_group, g.before.inodes_per_group,
            "re-deriving inodes per group would move every inode in the image"
        );
        assert_eq!(
            g.after.inode_table_blocks, g.before.inode_table_blocks,
            "a longer inode table would overwrite the blocks after it"
        );
    }

    #[test]
    fn a_request_the_image_already_meets_writes_nothing() {
        assert!(grown(10 * 1024 * 1024 * 1024, 10 * 1024 * 1024 * 1024).is_none());
        assert!(grown(10 * 1024 * 1024 * 1024, 1024 * 1024 * 1024).is_none());
    }

    #[test]
    fn a_grow_inside_one_group_appends_no_group_and_frees_the_padding() {
        let g = grown(130 * 1024 * 1024, 140 * 1024 * 1024).unwrap();
        assert_eq!(g.appended_groups, 2..2, "both sizes are two groups");
        let (tail, gained) = g.filled_tail.expect("the short tail grew");
        assert_eq!(tail, 1);
        assert_eq!(gained, 512..3072);
    }

    #[test]
    fn a_grow_across_groups_fills_the_old_tail_and_appends_the_rest() {
        let g = grown(130 * 1024 * 1024, 400 * 1024 * 1024).unwrap();
        assert_eq!(g.appended_groups, 2..4);
        let (tail, gained) = g.filled_tail.expect("the short tail grew");
        assert_eq!(tail, 1);
        assert_eq!(gained, 512..32768, "the old tail group is full now");
    }

    #[test]
    fn a_grow_onto_a_whole_group_boundary_fills_no_tail() {
        let g = grown(256 * 1024 * 1024, 384 * 1024 * 1024).unwrap();
        assert_eq!(g.appended_groups, 2..3);
        assert!(
            g.filled_tail.is_none(),
            "a group that was already whole gains nothing"
        );
    }

    #[test]
    fn a_tail_too_short_to_hold_its_own_metadata_is_left_out() {
        assert!(
            grown(256 * 1024 * 1024, 256 * 1024 * 1024 + 4096 * 8).is_none(),
            "eight blocks cannot hold a group's own metadata, so the filesystem gains nothing and the file keeps the slack"
        );
        let g = grown(256 * 1024 * 1024, 400 * 1024 * 1024).unwrap();
        assert!(
            g.after.blocks_in_group(g.after.num_groups - 1)
                > 1 + g.after.gdt_blocks
                    + g.after.reserved_gdt_blocks
                    + 2
                    + g.after.inode_table_blocks,
            "every group the grow keeps has room for its own metadata"
        );
    }

    /// The last reserved block is never spent: consuming it would leave the resize inode naming blocks the same grow just turned into descriptors, which e2fsck reads as illegal blocks in inode 7.
    #[test]
    fn a_grow_stops_one_block_short_of_spending_the_whole_reserved_run() {
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let reachable = u64::from(
            (plan.layout.gdt_blocks + plan.layout.reserved_gdt_blocks - 1)
                * DESCS_PER_BLOCK
                * BLOCKS_PER_GROUP,
        ) * u64::from(BLOCK_SIZE);

        let grown = plan_grow(&plan.superblock, reachable).unwrap().unwrap();
        assert!(
            grown.after.reserved_gdt_blocks >= 1,
            "the volume must still own a reserved block to keep its resize inode honest"
        );

        let err = plan_grow(&plan.superblock, reachable + BLOCK_SIZE as u64).unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("can grow to at most") && rendered.contains(&reachable.to_string()),
            "a refusal has to name the size the volume can still reach: {rendered}"
        );
    }

    #[test]
    fn a_grow_consumes_reserved_blocks_only_as_the_table_needs_them() {
        let plan = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let g = plan_grow(&plan.superblock, 100 * 1024 * 1024 * 1024)
            .unwrap()
            .unwrap();
        assert_eq!(g.after.gdt_blocks, 7, "800 groups need seven blocks of 128");
        assert_eq!(
            g.after.gdt_blocks + g.after.reserved_gdt_blocks,
            g.before.gdt_blocks + g.before.reserved_gdt_blocks,
            "the run the resize inode owns never changes length, only its split"
        );
        assert_eq!(g.superblock.reserved_gdt_blocks, 633);
    }

    #[test]
    fn a_grow_updates_the_counts_the_superblock_publishes() {
        let g = grown(130 * 1024 * 1024, 400 * 1024 * 1024).unwrap();
        assert_eq!(g.superblock.blocks_count, g.after.block_count);
        assert_eq!(g.superblock.inodes_count, g.after.inodes_count);
        assert!(
            g.superblock.inodes_count > g.before.inodes_count,
            "appended groups bring their own inodes"
        );
    }
}
