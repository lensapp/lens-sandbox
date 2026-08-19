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
    pub reserved_gdt_first_block: Option<u32>,
    pub reserved_gdt_blocks: u32,
    pub bbmp_block: u32,
    pub ibmp_block: u32,
    pub itab_first_block: u32,
    pub data_first_block: u32,
}

pub fn group_block_layout(layout: &Layout, g: u32) -> GroupBlockLayout {
    let group_base = g * layout.blocks_per_group;
    let has_backup = layout.group_has_backup(g);
    let gdt_blocks = layout.gdt_blocks;

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
    let reserved_gdt_first_block = if has_backup && layout.reserved_gdt_blocks > 0 {
        let b = group_base + off;
        off += layout.reserved_gdt_blocks;
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
        reserved_gdt_first_block,
        reserved_gdt_blocks: layout.reserved_gdt_blocks,
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
    if let Some(reserved) = gbl.reserved_gdt_first_block {
        for i in 0..gbl.reserved_gdt_blocks {
            mark(&mut bm, reserved + i);
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
        let journal_first = journal_first_block(layout);
        for i in 0..journal_blocks(layout) {
            mark(&mut bm, journal_first + i);
        }
        if layout.reserved_gdt_blocks > 0 {
            mark(&mut bm, resize_dind_block(layout));
        }
    }

    let real_blocks = layout.blocks_in_group(g) as usize;
    let bitmap_bits = (BLOCK_SIZE * 8) as usize;
    for i in real_blocks..bitmap_bits {
        set_bit(&mut bm, i);
    }

    bm
}

pub fn journal_first_block(layout: &Layout) -> u32 {
    group_block_layout(layout, 0).data_first_block + 1 + LOST_FOUND_BLOCKS
}

pub fn resize_dind_block(layout: &Layout) -> u32 {
    journal_first_block(layout) + journal_blocks(layout)
}

fn group_zero_free_blocks(layout: &Layout) -> u32 {
    let gbl = group_block_layout(layout, 0);
    layout
        .blocks_in_group(0)
        .saturating_sub(gbl.data_first_block + 1 + LOST_FOUND_BLOCKS + RESIZE_DIND_BLOCKS)
}

pub fn journal_blocks(layout: &Layout) -> u32 {
    if group_zero_free_blocks(layout) >= JOURNAL_TARGET_BLOCKS * 4 {
        JOURNAL_TARGET_BLOCKS
    } else {
        JBD2_MIN_JOURNAL_BLOCKS
    }
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
    pub journal_inode: Inode,
    pub resize_inode: Inode,
    pub root_dir_block: Vec<u8>,
    pub lost_found_blocks: Vec<Vec<u8>>,
}

/// The double-indirect block and the reserved GDT blocks it points at, each of which is itself the indirect block listing its own backup copies.
pub struct ResizeBlocks {
    pub dind_block: u32,
    pub dind_contents: Vec<u8>,
    pub indirect: Vec<(u32, Vec<u8>)>,
}

fn backup_groups_above_zero(layout: &Layout) -> Vec<u32> {
    layout
        .backup_groups
        .iter()
        .copied()
        .filter(|g| *g > 0)
        .collect()
}

/// The resize inode's block tree as e2fsck walks it: DIND slot `gdt_blocks + k` names reserved block `k`, and that block lists its copy in every backup group above 0.
pub fn resize_blocks(layout: &Layout, dind_block: u32) -> Option<ResizeBlocks> {
    if layout.reserved_gdt_blocks == 0 {
        return None;
    }
    let first_reserved = group_block_layout(layout, 0)
        .reserved_gdt_first_block
        .expect("group 0 always carries a backup, so it holds the primary reserved run");
    let backups = backup_groups_above_zero(layout);

    let mut dind_contents = vec![0u8; BLOCK_SIZE as usize];
    let dind_slot_base = layout.gdt_blocks % ADDRS_PER_BLOCK;
    let mut indirect = Vec::with_capacity(layout.reserved_gdt_blocks as usize);

    for k in 0..layout.reserved_gdt_blocks {
        let primary = first_reserved + k;
        let slot = (dind_slot_base + k) as usize;
        dind_contents[slot * 4..slot * 4 + 4].copy_from_slice(&primary.to_le_bytes());

        let mut contents = vec![0u8; BLOCK_SIZE as usize];
        for (i, g) in backups.iter().enumerate() {
            let copy = primary + g * layout.blocks_per_group;
            contents[i * 4..i * 4 + 4].copy_from_slice(&copy.to_le_bytes());
        }
        indirect.push((primary, contents));
    }

    Some(ResizeBlocks {
        dind_block,
        dind_contents,
        indirect,
    })
}

impl Plan {
    pub fn new(
        image_size_bytes: u64,
        uuid: [u8; 16],
        volume_name: &str,
        mkfs_time: u32,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            image_size_bytes >= BLOCK_SIZE as u64,
            "an image of {image_size_bytes} bytes is too small for a journalled volume: it holds no whole {BLOCK_SIZE}-byte block"
        );
        let layout = Layout::for_fresh_image(image_size_bytes);
        let free = group_zero_free_blocks(&layout);
        anyhow::ensure!(
            free >= JBD2_MIN_JOURNAL_BLOCKS * 4,
            "an image of {image_size_bytes} bytes is too small for a journalled volume: group 0 has {free} free blocks, and the smallest recoverable journal needs {}",
            JBD2_MIN_JOURNAL_BLOCKS * 4
        );

        let root_data_block = group_block_layout(&layout, 0).data_first_block;
        let lost_found_first_block = root_data_block + 1;

        let root_inode = Inode::root_directory(mkfs_time, root_data_block);
        let lost_found_inode = Inode::lost_found_directory(mkfs_time, lost_found_first_block);
        let journal_inode = Inode::journal(
            mkfs_time,
            journal_first_block(&layout),
            journal_blocks(&layout),
        );

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

        let resize_inode = Inode::resize(
            mkfs_time,
            resize_dind_block(&layout),
            layout.reserved_gdt_blocks,
            backup_groups_above_zero(&layout).len() as u32,
        );

        let mut superblock = Superblock::for_fresh_image(&layout, uuid, volume_name, mkfs_time);
        let (free_blocks, free_inodes) = total_free_counts(&layout);
        superblock.free_blocks_count = free_blocks;
        superblock.free_inodes_count = free_inodes;

        Ok(Self {
            layout,
            superblock,
            root_inode,
            lost_found_inode,
            journal_inode,
            resize_inode,
            root_dir_block,
            lost_found_blocks,
        })
    }

    pub fn root_data_block(&self) -> u32 {
        group_block_layout(&self.layout, 0).data_first_block
    }

    pub fn lost_found_first_block(&self) -> u32 {
        self.root_data_block() + 1
    }

    pub fn journal_first_block(&self) -> u32 {
        journal_first_block(&self.layout)
    }

    pub fn journal_blocks(&self) -> u32 {
        journal_blocks(&self.layout)
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
        Layout::for_fresh_image(32 * 1024 * 1024)
    }

    fn ten_gib() -> Layout {
        Layout::for_fresh_image(10 * 1024 * 1024 * 1024)
    }

    #[test]
    fn group_block_layout_group_0_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 0);
        assert_eq!(gbl.group_base, 0);
        assert_eq!(gbl.sb_block, Some(0));
        assert_eq!(gbl.gdt_first_block, Some(1));
        assert_eq!(gbl.gdt_blocks, 1);
        assert_eq!(gbl.reserved_gdt_first_block, Some(2));
        assert_eq!(gbl.reserved_gdt_blocks, 639);
        assert_eq!(gbl.bbmp_block, 641);
        assert_eq!(gbl.ibmp_block, 642);
        assert_eq!(gbl.itab_first_block, 643);
        assert_eq!(gbl.data_first_block, 643 + 512);
    }

    #[test]
    fn group_block_layout_backup_group_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 1);
        assert_eq!(gbl.group_base, 32768);
        assert_eq!(gbl.sb_block, Some(32768));
        assert_eq!(gbl.gdt_first_block, Some(32769));
        assert_eq!(gbl.reserved_gdt_first_block, Some(32770));
        assert_eq!(gbl.bbmp_block, 33409);
        assert_eq!(gbl.ibmp_block, 33410);
        assert_eq!(gbl.itab_first_block, 33411);
        assert_eq!(gbl.data_first_block, 33411 + 512);
    }

    #[test]
    fn group_block_layout_non_backup_group_10_gib() {
        let l = ten_gib();
        let gbl = group_block_layout(&l, 2);
        assert_eq!(gbl.group_base, 65536);
        assert_eq!(gbl.sb_block, None);
        assert_eq!(gbl.gdt_first_block, None);
        assert_eq!(gbl.reserved_gdt_first_block, None);
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
        assert_eq!(gbl.reserved_gdt_blocks, 7);
        assert_eq!(gbl.bbmp_block, 9);
        assert_eq!(gbl.ibmp_block, 10);
        assert_eq!(gbl.itab_first_block, 11);
        assert_eq!(gbl.data_first_block, 11 + 128);
    }

    #[test]
    fn block_bitmap_group_0_marks_metadata_and_root_and_lf() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 0);

        for bit in 0..=1154 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set (metadata)");
        }
        for bit in 1155..=1159 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set (root/l+f)");
        }
        for bit in 1160..=5255 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set (journal)");
        }
        assert!(get_bit(&bm, 5256), "bit 5256 should be set (resize dind)");
        for bit in 5257..32768 {
            assert!(!get_bit(&bm, bit), "bit {bit} should be clear (free)");
        }
    }

    #[test]
    fn block_bitmap_backup_group_marks_only_metadata() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 1);
        for bit in 0..=1154 {
            assert!(get_bit(&bm, bit), "bit {bit} should be set");
        }
        for bit in 1155..32768 {
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
        let l = Layout::for_fresh_image(40 * 1024 * 1024);
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
    fn a_ten_gib_image_gets_the_full_journal_target() {
        let l = ten_gib();
        let blocks = journal_blocks(&l);
        assert_eq!(blocks, JOURNAL_TARGET_BLOCKS);
        assert!(
            blocks < group_zero_free_blocks(&l),
            "the journal has to fit group 0's free space alongside root and lost+found"
        );
        assert!(
            blocks <= 32768,
            "one initialised extent record caps at 32768 blocks, and the journal must stay a single extent"
        );
    }

    #[test]
    fn a_thirty_two_mib_image_falls_back_to_the_jbd2_minimum() {
        let l = small();
        assert_eq!(journal_blocks(&l), JBD2_MIN_JOURNAL_BLOCKS);
    }

    /// The document validator refuses a smaller disk than this, so anything it admits has to format — otherwise a valid document fails at provisioning with an error about block groups.
    #[test]
    fn the_smallest_size_a_document_may_declare_is_a_size_this_writer_can_format() {
        let smallest = lns_artifact::disk::MIN_MIB * 1024 * 1024;
        Plan::new(smallest, [0; 16], "test", 0)
            .expect("the validator's minimum must be formattable");
    }

    #[test]
    fn an_image_too_small_to_hold_the_minimum_journal_is_refused() {
        let err = Plan::new(4 * 1024 * 1024, [0; 16], "test", 0).unwrap_err();
        assert!(
            err.to_string()
                .contains("too small for a journalled volume"),
            "volume provisioning runs Plan::new on a service worker, so an unformattable size has to come back as an error: {err}"
        );
    }

    #[test]
    fn the_guard_sits_on_the_real_boundary_rather_than_a_threshold_above_it() {
        let smallest_accepted = (4..=64u64)
            .map(|mib| mib * 1024 * 1024)
            .find(|size| Plan::new(*size, [0; 16], "test", 0).is_ok())
            .expect("some size in 4..=64 MiB is formattable");
        assert!(
            Plan::new(smallest_accepted - 1024 * 1024, [0; 16], "test", 0).is_err(),
            "one MiB below the smallest accepted size must still be refused"
        );
    }

    #[test]
    fn the_journal_never_exceeds_a_quarter_of_group_zeros_free_space() {
        for size in (4 * 1024 * 1024..=10 * 1024 * 1024 * 1024u64).step_by(7 * 1024 * 1024) {
            let layout = Layout::for_fresh_image(size);
            let free = group_zero_free_blocks(&layout);
            if free < JBD2_MIN_JOURNAL_BLOCKS * 4 {
                continue;
            }
            assert!(
                journal_blocks(&layout) * 4 <= free,
                "{size} bytes: journal_blocks answers unconditionally, so everything the guard admits must have the room"
            );
        }
    }

    #[test]
    fn the_journal_sits_immediately_after_lost_and_found() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(
            p.journal_first_block(),
            p.lost_found_first_block() + LOST_FOUND_BLOCKS
        );
    }

    #[test]
    fn group_zero_block_bitmap_marks_every_journal_block_and_no_more() {
        let l = ten_gib();
        let bm = block_bitmap(&l, 0);
        let first = journal_first_block(&l) as usize;
        let len = journal_blocks(&l) as usize;
        for bit in first..first + len {
            assert!(get_bit(&bm, bit), "journal bit {bit} should be set");
        }
        assert!(
            !get_bit(&bm, first + len + RESIZE_DIND_BLOCKS as usize),
            "a journal that runs past its reservation would be overwritten by the first file written"
        );
    }

    #[test]
    fn an_image_smaller_than_group_zeros_own_metadata_is_refused_not_wrapped() {
        for bytes in [1024u64, 3072, 32 * 1024, 16 * 1024 * 1024] {
            let err = Plan::new(bytes, [0; 16], "test", 0)
                .expect_err("an image this small cannot hold a journalled filesystem");
            assert!(
                format!("{err:#}").contains("too small for a journalled volume"),
                "group 0's metadata outgrows its block count here, so an unguarded subtraction \
                 would wrap to a huge free count and admit the image: {err:#}"
            );
        }
    }

    #[test]
    fn group_zero_free_blocks_drop_by_exactly_the_journal_size() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 0);
        assert_eq!(gd.free_blocks_count, 31607 - 4096);
    }

    #[test]
    fn the_journal_does_not_move_the_root_or_lost_and_found_data_blocks() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(p.root_data_block(), 1155);
        assert_eq!(p.lost_found_first_block(), 1156);
    }

    #[test]
    fn group_descriptor_group_0_10_gib() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 0);
        assert_eq!(gd.free_blocks_count, 27511);
        assert_eq!(gd.free_inodes_count, 8181);
        assert_eq!(gd.used_dirs_count, 2);
        assert_eq!(gd.block_bitmap, 641);
        assert_eq!(gd.inode_bitmap, 642);
        assert_eq!(gd.inode_table, 643);
    }

    #[test]
    fn group_descriptor_backup_group_10_gib() {
        let l = ten_gib();
        let gd = group_descriptor(&l, 1);
        assert_eq!(gd.free_blocks_count, 31613);
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
        let p = Plan::new(32 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(
            p.superblock.free_blocks_count,
            8047 - JBD2_MIN_JOURNAL_BLOCKS
        );
        assert_eq!(p.superblock.free_inodes_count, 2048 - 11);
    }

    #[test]
    fn plan_10_gib_total_free_counts() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let backup_groups = 9u32;
        let other_groups = 80 - 9;
        let expected_metadata = backup_groups * (516 + 639)
            + other_groups * 514
            + 1 /*root dir*/
            + 4 /*l+f*/
            + JOURNAL_TARGET_BLOCKS
            + RESIZE_DIND_BLOCKS;
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
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(p.root_data_block(), 1155);
        let (logical, length, physical) = first_extent(&p.root_inode);
        assert_eq!(logical, 0);
        assert_eq!(length, 1);
        assert_eq!(physical, 1155);
    }

    #[test]
    fn plan_lost_found_inode_points_to_4_consecutive_blocks() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(p.lost_found_first_block(), 1156);
        let (logical, length, physical) = first_extent(&p.lost_found_inode);
        assert_eq!(logical, 0);
        assert_eq!(length, 4);
        assert_eq!(physical, 1156);
    }

    #[test]
    fn plan_journal_inode_covers_exactly_the_reserved_run() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let (logical, length, physical) = first_extent(&p.journal_inode);
        assert_eq!(logical, 0);
        assert_eq!(length as u32, p.journal_blocks());
        assert_eq!(physical as u32, p.journal_first_block());
    }

    #[test]
    fn plan_lost_found_blocks_count_matches_constant() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(p.lost_found_blocks.len(), LOST_FOUND_BLOCKS as usize);
        for block in &p.lost_found_blocks {
            assert_eq!(block.len(), BLOCK_SIZE as usize);
        }
    }

    #[test]
    fn plan_root_dir_block_has_three_entries() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
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
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let b = &p.lost_found_blocks[0];
        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), LOST_FOUND_INO);
        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), ROOT_INO);
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 4084);
    }

    #[test]
    fn plan_lost_found_trailing_blocks_are_sentinels() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        for i in 1..LOST_FOUND_BLOCKS as usize {
            let b = &p.lost_found_blocks[i];
            assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 0);
            assert_eq!(u16::from_le_bytes([b[4], b[5]]), BLOCK_SIZE as u16);
        }
    }

    fn slot(block: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ])
    }

    #[test]
    fn the_dind_block_names_every_reserved_gdt_block_at_the_slot_e2fsck_reads() {
        let l = ten_gib();
        let r = resize_blocks(&l, resize_dind_block(&l)).unwrap();
        assert_eq!(r.dind_block, 5256);
        assert_eq!(
            slot(&r.dind_contents, 0),
            0,
            "the slots below gdt_blocks belong to the live table, not the reserved run"
        );
        for k in 0..l.reserved_gdt_blocks {
            assert_eq!(
                slot(&r.dind_contents, (l.gdt_blocks + k) as usize),
                2 + k,
                "reserved block {k} must sit at the slot e2fsck computes from gdt_blocks"
            );
        }
    }

    #[test]
    fn each_reserved_gdt_block_lists_its_own_copy_in_every_backup_group() {
        let l = ten_gib();
        let r = resize_blocks(&l, resize_dind_block(&l)).unwrap();
        let backups = backup_groups_above_zero(&l);
        assert_eq!(backups, vec![1, 3, 5, 7, 9, 25, 27, 49]);

        let (primary, contents) = &r.indirect[0];
        assert_eq!(*primary, 2);
        for (i, g) in backups.iter().enumerate() {
            assert_eq!(
                slot(contents, i),
                2 + g * l.blocks_per_group,
                "backup {g} of reserved block 2"
            );
        }
        assert_eq!(
            slot(contents, backups.len()),
            0,
            "a trailing entry would be walked as a block and refused as out of range"
        );
    }

    #[test]
    fn the_resize_inode_counts_every_block_pass_1_will_visit() {
        let l = ten_gib();
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        let backups = backup_groups_above_zero(&l).len() as u32;
        let visited = 1 + l.reserved_gdt_blocks + l.reserved_gdt_blocks * backups;
        assert_eq!(
            p.resize_inode.blocks,
            visited * 8,
            "e2fsck counts the dind block, every reserved block, and every backup entry, and refuses any other i_blocks"
        );
        assert_eq!(
            p.resize_inode.flags, 0,
            "the resize inode is block-mapped; an extents flag makes e2fsck read the dind pointer as an extent header"
        );
    }

    #[test]
    fn an_image_with_no_room_to_grow_reserves_nothing_and_writes_no_resize_tree() {
        let mut l = ten_gib();
        l.reserved_gdt_blocks = 0;
        assert!(
            resize_blocks(&l, resize_dind_block(&l)).is_none(),
            "reserved blocks and the resize inode arrive together or not at all"
        );
    }

    #[test]
    fn an_image_with_no_room_to_grow_does_not_advertise_a_resize_inode_it_never_wrote() {
        let mut l = ten_gib();
        l.reserved_gdt_blocks = 0;
        let sb = Superblock::for_fresh_image(&l, [0; 16], "test", 0);
        assert_eq!(
            sb.feature_compat & FEATURE_COMPAT_RESIZE_INODE,
            0,
            "e2fsck reads the feature bit as a promise that inode 7 holds a block tree, and refuses a zeroed one"
        );
    }

    #[test]
    fn a_volume_past_one_descriptor_block_still_addresses_its_reserved_run() {
        for gib in [17u64, 20, 64, 1024] {
            let l = Layout::for_fresh_image(gib * 1024 * 1024 * 1024);
            assert!(l.gdt_blocks > 1, "{gib} GiB needs more than one GDT block");
            let r = resize_blocks(&l, resize_dind_block(&l))
                .expect("a volume this size still reserves room to grow");
            let highest = (l.gdt_blocks + l.reserved_gdt_blocks - 1) as usize;
            assert!(
                highest < ADDRS_PER_BLOCK as usize,
                "{gib} GiB: slot {highest} would be written past the end of the double-indirect block"
            );
            assert_eq!(
                slot(&r.dind_contents, highest),
                r.indirect.last().expect("a reserved run").0,
                "{gib} GiB: the last reserved block must sit at the last slot the tree uses"
            );
        }
    }

    #[test]
    fn plan_layout_geometry_propagates_to_superblock() {
        let p = Plan::new(10 * 1024 * 1024 * 1024, [0; 16], "test", 0).unwrap();
        assert_eq!(p.superblock.blocks_count, p.layout.block_count);
        assert_eq!(p.superblock.inodes_count, p.layout.inodes_count);
        assert_eq!(p.superblock.blocks_per_group, p.layout.blocks_per_group);
        assert_eq!(p.superblock.inodes_per_group, p.layout.inodes_per_group);
    }
}
