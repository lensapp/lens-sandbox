use crate::upperfs::constants::*;
use crate::upperfs::dir::{DirBlock, DirEntry};
use crate::upperfs::format::{GroupDescriptor, Inode, Superblock};
use crate::upperfs::layout::Layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupBlockLayout {
    pub group_base: u32,
    pub sb_block: Option<u32>,
    pub gdt_first_block: Option<u32>,
    pub gdt_blocks: u32,
    pub bbmp_block: u32,
    pub ibmp_block: u32,
    pub itab_first_block: u32,
    pub data_first_block: u32,
}

pub fn group_block_layout(layout: &Layout, g: u32) -> GroupBlockLayout {
    let group_base = g * layout.blocks_per_group;
    let has_backup = layout.group_has_backup(g);
    let gdt_blocks = layout.num_groups.div_ceil(BLOCK_SIZE / GROUP_DESC_SIZE);

    let mut off = 0u32;
    let sb_block = if has_backup {
        let b = group_base + off;
        off += 1;
        Some(b)
    } else {
        None
    };
    let gdt_first_block = if has_backup {
        let b = group_base + off;
        off += gdt_blocks;
        Some(b)
    } else {
        None
    };
    let bbmp_block = group_base + off;
    off += 1;
    let ibmp_block = group_base + off;
    off += 1;
    let itab_first_block = group_base + off;
    off += layout.inode_table_blocks;
    let data_first_block = group_base + off;

    GroupBlockLayout {
        group_base,
        sb_block,
        gdt_first_block,
        gdt_blocks,
        bbmp_block,
        ibmp_block,
        itab_first_block,
        data_first_block,
    }
}

pub fn block_bitmap(layout: &Layout, g: u32) -> Vec<u8> {
    let mut bm = vec![0u8; BLOCK_SIZE as usize];
    let gbl = group_block_layout(layout, g);

    let mark = |bm: &mut [u8], abs_block: u32| {
        let local = (abs_block - gbl.group_base) as usize;
        set_bit(bm, local);
    };

    if let Some(sb) = gbl.sb_block {
        mark(&mut bm, sb);
    }
    if let Some(gdt) = gbl.gdt_first_block {
        for i in 0..gbl.gdt_blocks {
            mark(&mut bm, gdt + i);
        }
    }
    mark(&mut bm, gbl.bbmp_block);
    mark(&mut bm, gbl.ibmp_block);
    for i in 0..layout.inode_table_blocks {
        mark(&mut bm, gbl.itab_first_block + i);
    }

    if g == 0 {
        mark(&mut bm, gbl.data_first_block);
        for i in 0..LOST_FOUND_BLOCKS {
            mark(&mut bm, gbl.data_first_block + 1 + i);
        }
    }

    let real_blocks = layout.blocks_in_group(g) as usize;
    let bitmap_bits = (BLOCK_SIZE * 8) as usize;
    for i in real_blocks..bitmap_bits {
        set_bit(&mut bm, i);
    }

    bm
}

pub fn inode_bitmap(layout: &Layout, g: u32) -> Vec<u8> {
    let mut bm = vec![0u8; BLOCK_SIZE as usize];

    if g == 0 {
        for n in 1..=11usize {
            set_bit(&mut bm, n - 1);
        }
    }

    let real_inodes = layout.inodes_per_group as usize;
    let bitmap_bits = (BLOCK_SIZE * 8) as usize;
    for i in real_inodes..bitmap_bits {
        set_bit(&mut bm, i);
    }

    bm
}

pub fn group_descriptor(layout: &Layout, g: u32) -> GroupDescriptor {
    let gbl = group_block_layout(layout, g);
    let bbmp = block_bitmap(layout, g);
    let ibmp = inode_bitmap(layout, g);

    let blocks_in_group = layout.blocks_in_group(g) as usize;
    let allocated_blocks = count_set_bits(&bbmp, 0, blocks_in_group);
    let free_blocks = blocks_in_group - allocated_blocks;

    let real_inodes = layout.inodes_per_group as usize;
    let allocated_inodes = count_set_bits(&ibmp, 0, real_inodes);
    let free_inodes = real_inodes - allocated_inodes;

    GroupDescriptor {
        block_bitmap: gbl.bbmp_block,
        inode_bitmap: gbl.ibmp_block,
        inode_table: gbl.itab_first_block,
        free_blocks_count: u16::try_from(free_blocks).expect("free_blocks fits u16"),
        free_inodes_count: u16::try_from(free_inodes).expect("free_inodes fits u16"),
        used_dirs_count: if g == 0 { 2 } else { 0 },
        flags: 0,
        itable_unused: 0,
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub layout: Layout,
    pub superblock: Superblock,
    pub root_inode: Inode,
    pub lost_found_inode: Inode,
    pub root_dir_block: Vec<u8>,
    pub lost_found_blocks: Vec<Vec<u8>>,
}

impl Plan {
    pub fn new(image_size_bytes: u64, uuid: [u8; 16], volume_name: &str, mkfs_time: u32) -> Self {
        let layout = Layout::from_image_size(image_size_bytes);

        let root_data_block = group_block_layout(&layout, 0).data_first_block;
        let lost_found_first_block = root_data_block + 1;

        let root_inode = Inode::root_directory(mkfs_time, root_data_block);
        let lost_found_inode = Inode::lost_found_directory(mkfs_time, lost_found_first_block);

        let mut root_dir = DirBlock::new();
        root_dir.push(DirEntry::new(ROOT_INO, FT_DIR, "."));
        root_dir.push(DirEntry::new(ROOT_INO, FT_DIR, ".."));
        root_dir.push(DirEntry::new(LOST_FOUND_INO, FT_DIR, "lost+found"));
        let root_dir_block = root_dir.to_bytes(BLOCK_SIZE);

        let mut lf_first = DirBlock::new();
        lf_first.push(DirEntry::new(LOST_FOUND_INO, FT_DIR, "."));
        lf_first.push(DirEntry::new(ROOT_INO, FT_DIR, ".."));
        let mut lost_found_blocks = Vec::with_capacity(LOST_FOUND_BLOCKS as usize);
        lost_found_blocks.push(lf_first.to_bytes(BLOCK_SIZE));
        for _ in 1..LOST_FOUND_BLOCKS {
            lost_found_blocks.push(DirBlock::sentinel(BLOCK_SIZE));
        }

        let mut superblock = Superblock::for_fresh_image(&layout, uuid, volume_name, mkfs_time);
        let (free_blocks, free_inodes) = total_free_counts(&layout);
        superblock.free_blocks_count = free_blocks;
        superblock.free_inodes_count = free_inodes;

        Self {
            layout,
            superblock,
            root_inode,
            lost_found_inode,
            root_dir_block,
            lost_found_blocks,
        }
    }

    pub fn root_data_block(&self) -> u32 {
        group_block_layout(&self.layout, 0).data_first_block
    }

    pub fn lost_found_first_block(&self) -> u32 {
        self.root_data_block() + 1
    }
}

fn total_free_counts(layout: &Layout) -> (u32, u32) {
    let mut blocks = 0u32;
    let mut inodes = 0u32;
    for g in 0..layout.num_groups {
        let gd = group_descriptor(layout, g);
        blocks += gd.free_blocks_count as u32;
        inodes += gd.free_inodes_count as u32;
    }
    (blocks, inodes)
}

fn set_bit(buf: &mut [u8], bit: usize) {
    buf[bit / 8] |= 1u8 << (bit % 8);
}

fn get_bit(buf: &[u8], bit: usize) -> bool {
    (buf[bit / 8] >> (bit % 8)) & 1 == 1
}

fn count_set_bits(buf: &[u8], start: usize, end: usize) -> usize {
    (start..end).filter(|&i| get_bit(buf, i)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Layout {
        Layout::from_image_size(32 * 1024 * 1024)
    }

    fn ten_gib() -> Layout {
        Layout::from_image_size(10 * 1024 * 1024 * 1024)
    }

    #[test]
    fn group_block_layout_group_0_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 0);
        assert_eq!(gbl.group_base, 0);
        assert_eq!(gbl.sb_block, Some(0));
        assert_eq!(gbl.gdt_first_block, Some(1));
        assert_eq!(gbl.gdt_blocks, 1);
        assert_eq!(gbl.bbmp_block, 2);
        assert_eq!(gbl.ibmp_block, 3);
        assert_eq!(gbl.itab_first_block, 4);
        assert_eq!(gbl.data_first_block, 4 + 512);
    }

    #[test]
    fn group_block_layout_backup_group_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 1);
        assert_eq!(gbl.group_base, 32768);
        assert_eq!(gbl.sb_block, Some(32768));
        assert_eq!(gbl.gdt_first_block, Some(32769));
        assert_eq!(gbl.bbmp_block, 32770);
        assert_eq!(gbl.ibmp_block, 32771);
        assert_eq!(gbl.itab_first_block, 32772);
        assert_eq!(gbl.data_first_block, 32772 + 512);
    }

    #[test]
    fn group_block_layout_non_backup_group_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 2);
        assert_eq!(gbl.group_base, 65536);
        assert_eq!(gbl.sb_block, None);
        assert_eq!(gbl.gdt_first_block, None);
        assert_eq!(gbl.bbmp_block, 65536);
        assert_eq!(gbl.ibmp_block, 65537);
        assert_eq!(gbl.itab_first_block, 65538);
        assert_eq!(gbl.data_first_block, 65538 + 512);
    }

    #[test]
    fn group_block_layout_32_mib_single_group() {
        let l = small();
        let gbl = group_block_layout(&l, 0);
        assert_eq!(gbl.group_base, 0);
        assert_eq!(gbl.sb_block, Some(0));
        assert_eq!(gbl.gdt_first_block, Some(1));
        assert_eq!(gbl.bbmp_block, 2);
        assert_eq!(gbl.ibmp_block, 3);
        assert_eq!(gbl.itab_first_block, 4);
        assert_eq!(gbl.data_first_block, 4 + 128);
    }

    #[test]
    fn block_bitmap_group_0_marks_metadata_and_root_and_lf() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 0);

        for bit in 0..=515 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set (metadata)");
        }
        for bit in 516..=520 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set (root/l+f)");
        }
        for bit in 521..32768 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear (free)");
        }
    }

    #[test]
    fn block_bitmap_backup_group_marks_only_metadata() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 1);
        for bit in 0..=515 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set");
        }
        for bit in 516..32768 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear");
        }
    }

    #[test]
    fn block_bitmap_non_backup_group_marks_fewer_metadata_blocks() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 2);
        for bit in 0..=513 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set");
        }
        for bit in 514..32768 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear");
        }
    }

    #[test]
    fn block_bitmap_pads_past_real_blocks() {
        let l = Layout::from_image_size(40 * 1024 * 1024);
        assert_eq!(l.num_groups, 1);
        assert_eq!(l.last_group_blocks, 10240);
        let bm = block_bitmap(&l, 0);
        for bit in 10240..32768 {
            assert!(get_bit(&bm, bit), "padding bit {bit} must be set");
        }
    }

    #[test]
    fn inode_bitmap_group_0_marks_reserved_plus_lost_found() {
        let l = ten_gib();
        let bm = inode_bitmap(&l, 0);
        for bit in 0..=10 {
            assert!(get_bit(&bm, bit), "inode-bitmap bit {bit} should be set");
        }
        for bit in 11..8192 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear");
        }
    }

    #[test]
    fn inode_bitmap_non_zero_group_has_no_used_inodes() {
        let l = ten_gib();
        let bm = inode_bitmap(&l, 1);
        for bit in 0..8192 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear");
        }
    }

    #[test]
    fn inode_bitmap_pads_past_inodes_per_group() {
        let l = ten_gib();
        let bm = inode_bitmap(&l, 0);
        for bit in 8192..32768 {
            assert!(get_bit(&bm, bit), "padding bit {bit} must be set");
        }
    }

    #[test]
    fn group_descriptor_group_0_10_gib() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 0);
        assert_eq!(gd.free_blocks_count, 32247);
        assert_eq!(gd.free_inodes_count, 8181);
        assert_eq!(gd.used_dirs_count, 2);
        assert_eq!(gd.block_bitmap, 2);
        assert_eq!(gd.inode_bitmap, 3);
        assert_eq!(gd.inode_table, 4);
    }

    #[test]
    fn group_descriptor_backup_group_10_gib() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 1);
        assert_eq!(gd.free_blocks_count, 32252);
        assert_eq!(gd.free_inodes_count, 8192);
        assert_eq!(gd.used_dirs_count, 0);
    }

    #[test]
    fn group_descriptor_non_backup_group_10_gib() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 2);
        assert_eq!(gd.free_blocks_count, 32254);
        assert_eq!(gd.free_inodes_count, 8192);
        assert_eq!(gd.used_dirs_count, 0);
    }

    #[test]
    fn plan_32_mib_total_free_blocks() {
        let p = Plan::new(32 * 1024 * 1024, [0; 16], "test", 0);
        assert_eq!(p.superblock.free_blocks_count, 8055);
        assert_eq!(p.superblock.free_inodes_count, 2048 - 11);
    }

    #[test]
    fn plan_10_gib_total_free_counts() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        let backup_groups = 9u32;
        let other_groups = 80 - 9;
        let expected_metadata = backup_groups * 516 + other_groups * 514 + 1 /*root dir*/ + 4 /*l+f*/;
        let expected_free = 2_621_440 - expected_metadata;
        assert_eq!(p.superblock.free_blocks_count, expected_free);
        assert_eq!(p.superblock.free_inodes_count, 655_360 - 11);
    }

    fn first_extent(inode: &crate::upperfs::format::Inode) -> (u32, u16, u64) {
        let ib = &inode.i_block;
        let logical = u32::from_le_bytes([ib[12], ib[13], ib[14], ib[15]]);
        let length = u16::from_le_bytes([ib[16], ib[17]]);
        let hi = u16::from_le_bytes([ib[18], ib[19]]) as u64;
        let lo = u32::from_le_bytes([ib[20], ib[21], ib[22], ib[23]]) as u64;
        (logical, length, (hi << 32) | lo)
    }

    #[test]
    fn plan_root_inode_points_to_correct_block() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        assert_eq!(p.root_data_block(), 516);
        let (logical, length, physical) = first_extent(&p.root_inode);
        assert_eq!(logical, 0);
        assert_eq!(length, 1);
        assert_eq!(physical, 516);
    }

    #[test]
    fn plan_lost_found_inode_points_to_4_consecutive_blocks() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        assert_eq!(p.lost_found_first_block(), 517);
        let (logical, length, physical) = first_extent(&p.lost_found_inode);
        assert_eq!(logical, 0);
        assert_eq!(length, 4);
        assert_eq!(physical, 517);
    }

    #[test]
    fn plan_lost_found_blocks_count_matches_constant() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        assert_eq!(p.lost_found_blocks.len(), LOST_FOUND_BLOCKS as usize);
        for block in &p.lost_found_blocks {
            assert_eq!(block.len(), BLOCK_SIZE as usize);
        }
    }

    #[test]
    fn plan_root_dir_block_has_three_entries() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        let b = &p.root_dir_block;
        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), ROOT_INO);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 12);
        assert_eq!(b[6], 1);
        assert_eq!(&b[8..9], b".");
        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), ROOT_INO);
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 12);
        assert_eq!(
            u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
            LOST_FOUND_INO
        );
        assert_eq!(u16::from_le_bytes([b[28], b[29]]), 4072);
        assert_eq!(&b[32..42], b"lost+found");
    }

    #[test]
    fn plan_lost_found_first_block_has_dot_and_dotdot() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        let b = &p.lost_found_blocks[0];
        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), LOST_FOUND_INO);
        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), ROOT_INO);
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 4084);
    }

    #[test]
    fn plan_lost_found_trailing_blocks_are_sentinels() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        for i in 1..LOST_FOUND_BLOCKS as usize {
            let b = &p.lost_found_blocks[i];
            assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 0);
            assert_eq!(u16::from_le_bytes([b[4], b[5]]), BLOCK_SIZE as u16);
        }
    }

    #[test]
    fn plan_layout_geometry_propagates_to_superblock() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0);
        assert_eq!(p.superblock.blocks_count, p.layout.block_count);
        assert_eq!(p.superblock.inodes_count, p.layout.inodes_count);
        assert_eq!(p.superblock.blocks_per_group, p.layout.blocks_per_group);
        assert_eq!(p.superblock.inodes_per_group, p.layout.inodes_per_group);
    }
}
