use crate::upperfs::constants::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub block_count: u32,
    pub num_groups: u32,
    pub blocks_per_group: u32,
    pub last_group_blocks: u32,
    pub inodes_per_group: u32,
    pub inodes_count: u32,
    pub inode_table_blocks: u32,
    pub gdt_blocks: u32,
    pub reserved_gdt_blocks: u32,
    pub backup_groups: Vec<u32>,
}

impl Layout {
    pub fn for_fresh_image(image_size_bytes: u64) -> Self {
        let block_count = (image_size_bytes / BLOCK_SIZE as u64) as u32;
        let blocks_per_group = BLOCKS_PER_GROUP;
        let num_groups = block_count.div_ceil(blocks_per_group);
        let last_group_blocks = block_count - (num_groups - 1) * blocks_per_group;

        let inodes_target = (block_count as u64 * BLOCK_SIZE as u64 / INODE_RATIO as u64) as u32;
        let raw_per_group = inodes_target.div_ceil(num_groups);
        let align = BLOCK_SIZE / INODE_SIZE as u32;
        let inodes_per_group = raw_per_group.next_multiple_of(align);
        let inodes_count = inodes_per_group * num_groups;
        let inode_table_blocks = inodes_per_group * INODE_SIZE as u32 / BLOCK_SIZE;

        let backup_groups = compute_backup_groups(num_groups);
        let gdt_blocks = num_groups.div_ceil(DESCS_PER_BLOCK);
        let blocks_in_group_0 = if num_groups == 1 {
            last_group_blocks
        } else {
            blocks_per_group
        };
        let reserved_gdt_blocks = reserved_gdt_blocks(
            num_groups,
            gdt_blocks,
            blocks_in_group_0,
            inode_table_blocks,
        );

        Self {
            block_count,
            num_groups,
            blocks_per_group,
            last_group_blocks,
            inodes_per_group,
            inodes_count,
            inode_table_blocks,
            gdt_blocks,
            reserved_gdt_blocks,
            backup_groups,
        }
    }

    /// The geometry an image already has, which a grow MUST use: re-deriving it from the new size would move every inode.
    pub fn for_existing(
        block_count: u32,
        inodes_per_group: u32,
        gdt_blocks: u32,
        reserved_gdt_blocks: u32,
    ) -> Self {
        let blocks_per_group = BLOCKS_PER_GROUP;
        let num_groups = block_count.div_ceil(blocks_per_group);
        Self {
            block_count,
            num_groups,
            blocks_per_group,
            last_group_blocks: block_count - (num_groups - 1) * blocks_per_group,
            inodes_per_group,
            inodes_count: inodes_per_group * num_groups,
            inode_table_blocks: inodes_per_group * INODE_SIZE as u32 / BLOCK_SIZE,
            gdt_blocks,
            reserved_gdt_blocks,
            backup_groups: compute_backup_groups(num_groups),
        }
    }

    pub fn image_size_bytes(&self) -> u64 {
        self.block_count as u64 * BLOCK_SIZE as u64
    }

    pub fn group_has_backup(&self, g: u32) -> bool {
        self.backup_groups.binary_search(&g).is_ok()
    }

    pub fn blocks_in_group(&self, g: u32) -> u32 {
        if g + 1 == self.num_groups {
            self.last_group_blocks
        } else {
            self.blocks_per_group
        }
    }
}

/// Blocks held back after the GDT so a later grow can extend it without moving the bitmaps and inode tables that follow it in every group.
fn reserved_gdt_blocks(
    num_groups: u32,
    gdt_blocks: u32,
    blocks_in_group_0: u32,
    inode_table_blocks: u32,
) -> u32 {
    let grown_gdt =
        (u64::from(num_groups) * u64::from(GROWTH_FACTOR)).div_ceil(u64::from(DESCS_PER_BLOCK));
    // One double-indirect block addresses the live table and the reserved run together, so the two share its 1024 slots.
    let addressable = ADDRS_PER_BLOCK.saturating_sub(gdt_blocks);
    let wanted = grown_gdt
        .saturating_sub(u64::from(gdt_blocks))
        .min(u64::from(addressable)) as u32;

    let group_zero_fixed =
        1 + gdt_blocks + 2 + inode_table_blocks + 1 + LOST_FOUND_BLOCKS + RESIZE_DIND_BLOCKS;
    let spare = blocks_in_group_0
        .saturating_sub(group_zero_fixed)
        .saturating_sub(JBD2_MIN_JOURNAL_BLOCKS * 4);
    wanted.min(spare)
}

fn compute_backup_groups(num_groups: u32) -> Vec<u32> {
    let mut groups = std::collections::BTreeSet::new();
    groups.insert(0u32);
    if num_groups > 1 {
        groups.insert(1);
    }
    for base in [3u32, 5, 7] {
        let mut power: u64 = base as u64;
        while power < num_groups as u64 {
            groups.insert(power as u32);
            power = power.saturating_mul(base as u64);
        }
    }
    groups.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_32_mib() {
        let l = Layout::for_fresh_image(32 * 1024 * 1024);
        assert_eq!(l.block_count, 8192);
        assert_eq!(l.blocks_per_group, 32768);
        assert_eq!(l.num_groups, 1);
        assert_eq!(l.last_group_blocks, 8192);
        assert_eq!(l.inodes_per_group, 2048);
        assert_eq!(l.inodes_count, 2048);
        assert_eq!(l.inode_table_blocks, 128);
        assert_eq!(l.gdt_blocks, 1);
        assert_eq!(l.reserved_gdt_blocks, 7);
        assert_eq!(l.backup_groups, vec![0]);
    }

    #[test]
    fn layout_10_gib() {
        let l = Layout::for_fresh_image(10 * 1024 * 1024 * 1024);
        assert_eq!(l.block_count, 2_621_440);
        assert_eq!(l.num_groups, 80);
        assert_eq!(l.last_group_blocks, 32768);
        assert_eq!(l.inodes_per_group, 8192);
        assert_eq!(l.inodes_count, 655_360);
        assert_eq!(l.inode_table_blocks, 512);
        assert_eq!(l.gdt_blocks, 1);
        assert_eq!(l.reserved_gdt_blocks, 639);
        assert_eq!(l.backup_groups, vec![0, 1, 3, 5, 7, 9, 25, 27, 49]);
    }

    #[test]
    fn the_reserved_run_holds_room_for_a_thousandfold_growth() {
        let l = Layout::for_fresh_image(10 * 1024 * 1024 * 1024);
        let reachable_groups = (l.gdt_blocks + l.reserved_gdt_blocks) * DESCS_PER_BLOCK;
        assert!(
            reachable_groups >= l.num_groups * GROWTH_FACTOR,
            "a volume must be able to grow without ever moving the bitmaps that follow its descriptor table"
        );
    }

    #[test]
    fn the_reserved_run_never_crowds_out_the_journal() {
        for mib in [17u64, 18, 20, 24, 32, 48, 64, 128, 256] {
            let l = Layout::for_fresh_image(mib * 1024 * 1024);
            let group_zero = 1
                + l.gdt_blocks
                + l.reserved_gdt_blocks
                + 2
                + l.inode_table_blocks
                + 1
                + LOST_FOUND_BLOCKS
                + RESIZE_DIND_BLOCKS;
            assert!(
                l.blocks_in_group(0) >= group_zero + JBD2_MIN_JOURNAL_BLOCKS * 4,
                "{mib} MiB: reserving growth room must never cost the journal its blocks"
            );
        }
    }

    /// 16 MiB cannot hold a recoverable journal even with nothing reserved, so `Plan::new` refuses it; every larger size must survive the reservation.
    #[test]
    fn the_reservation_never_turns_a_formattable_size_into_an_unformattable_one() {
        for mib in [17u64, 18, 19, 20, 33, 129, 1025] {
            let l = Layout::for_fresh_image(mib * 1024 * 1024);
            let group_zero = 1
                + l.gdt_blocks
                + l.reserved_gdt_blocks
                + 2
                + l.inode_table_blocks
                + 1
                + LOST_FOUND_BLOCKS
                + RESIZE_DIND_BLOCKS;
            assert!(
                l.blocks_in_group(0) >= group_zero + JBD2_MIN_JOURNAL_BLOCKS * 4,
                "{mib} MiB formats without a reserved run, so it must still format with one"
            );
        }
    }

    #[test]
    fn the_live_table_and_the_reserved_run_share_one_block_of_addresses() {
        for gib in [1u64, 8, 16, 17, 20, 64, 256, 1024, 4096, 8192, 16383] {
            let l = Layout::for_fresh_image(gib * 1024 * 1024 * 1024);
            assert!(
                l.gdt_blocks + l.reserved_gdt_blocks <= ADDRS_PER_BLOCK,
                "{gib} GiB: one double-indirect block addresses the descriptor table and its reserved run together, so the two cannot outgrow a block of addresses"
            );
        }
    }

    #[test]
    fn image_size_rounds_down_to_block() {
        let l = Layout::for_fresh_image(32 * 1024 * 1024 + 1);
        assert_eq!(l.block_count, 8192);
        assert_eq!(l.image_size_bytes(), 32 * 1024 * 1024);
    }

    #[test]
    fn backup_groups_omits_1_for_single_group_image() {
        let l = Layout::for_fresh_image(32 * 1024 * 1024);
        assert_eq!(l.num_groups, 1);
        assert_eq!(l.backup_groups, vec![0]);
    }

    #[test]
    fn backup_groups_includes_1_for_multi_group_image() {
        let l = Layout::for_fresh_image(256 * 1024 * 1024);
        assert_eq!(l.num_groups, 2);
        assert!(l.group_has_backup(0));
        assert!(l.group_has_backup(1));
    }

    #[test]
    fn blocks_in_group_handles_short_tail() {
        let l = Layout::for_fresh_image(130 * 1024 * 1024);
        assert_eq!(l.num_groups, 2);
        assert_eq!(l.blocks_in_group(0), 32768);
        assert_eq!(l.blocks_in_group(1), 512);
    }

    #[test]
    fn group_has_backup_uses_binary_search() {
        let l = Layout::for_fresh_image(10 * 1024 * 1024 * 1024);
        for g in [0, 1, 3, 5, 7, 9, 25, 27, 49] {
            assert!(l.group_has_backup(g), "group {g} should have backup");
        }
        for g in [2, 4, 6, 8, 10, 11, 26, 48, 50, 79] {
            assert!(!l.group_has_backup(g), "group {g} should NOT have backup");
        }
    }
}
