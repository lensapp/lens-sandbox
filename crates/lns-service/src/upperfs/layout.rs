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
    pub backup_groups: Vec<u32>,
}

impl Layout {
    pub fn from_image_size(image_size_bytes: u64) -> Self {
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

        Self {
            block_count,
            num_groups,
            blocks_per_group,
            last_group_blocks,
            inodes_per_group,
            inodes_count,
            inode_table_blocks,
            backup_groups,
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
        let l = Layout::from_image_size(32 * 1024 * 1024);
        assert_eq!(l.block_count, 8192);
        assert_eq!(l.blocks_per_group, 32768);
        assert_eq!(l.num_groups, 1);
        assert_eq!(l.last_group_blocks, 8192);
        assert_eq!(l.inodes_per_group, 2048);
        assert_eq!(l.inodes_count, 2048);
        assert_eq!(l.inode_table_blocks, 128);
        assert_eq!(l.backup_groups, vec![0]);
    }

    #[test]
    fn layout_10_gib() {
        let l = Layout::from_image_size(10 * 1024 * 1024 * 1024);
        assert_eq!(l.block_count, 2_621_440);
        assert_eq!(l.num_groups, 80);
        assert_eq!(l.last_group_blocks, 32768);
        assert_eq!(l.inodes_per_group, 8192);
        assert_eq!(l.inodes_count, 655_360);
        assert_eq!(l.inode_table_blocks, 512);
        assert_eq!(l.backup_groups, vec![0, 1, 3, 5, 7, 9, 25, 27, 49]);
    }

    #[test]
    fn image_size_rounds_down_to_block() {
        let l = Layout::from_image_size(32 * 1024 * 1024 + 1);
        assert_eq!(l.block_count, 8192);
        assert_eq!(l.image_size_bytes(), 32 * 1024 * 1024);
    }

    #[test]
    fn backup_groups_omits_1_for_single_group_image() {
        let l = Layout::from_image_size(32 * 1024 * 1024);
        assert_eq!(l.num_groups, 1);
        assert_eq!(l.backup_groups, vec![0]);
    }

    #[test]
    fn backup_groups_includes_1_for_multi_group_image() {
        let l = Layout::from_image_size(256 * 1024 * 1024);
        assert_eq!(l.num_groups, 2);
        assert!(l.group_has_backup(0));
        assert!(l.group_has_backup(1));
    }

    #[test]
    fn blocks_in_group_handles_short_tail() {
        let l = Layout::from_image_size(130 * 1024 * 1024);
        assert_eq!(l.num_groups, 2);
        assert_eq!(l.blocks_in_group(0), 32768);
        assert_eq!(l.blocks_in_group(1), 512);
    }

    #[test]
    fn group_has_backup_uses_binary_search() {
        let l = Layout::from_image_size(10 * 1024 * 1024 * 1024);
        for g in [0, 1, 3, 5, 7, 9, 25, 27, 49] {
            assert!(l.group_has_backup(g), "group {g} should have backup");
        }
        for g in [2, 4, 6, 8, 10, 11, 26, 48, 50, 79] {
            assert!(!l.group_has_backup(g), "group {g} should NOT have backup");
        }
    }
}
