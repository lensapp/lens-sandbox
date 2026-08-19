use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::upperfs::constants::*;
use crate::upperfs::journal;
use crate::upperfs::plan::{
    Plan, block_bitmap, group_block_layout, group_descriptor, inode_bitmap, resize_blocks,
};

pub fn write_ext4(plan: &Plan, path: &Path) -> Result<()> {
    let tmp = tmp_path(path);
    let _ = std::fs::remove_file(&tmp);

    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;

    f.set_len(plan.layout.image_size_bytes())
        .with_context(|| format!("set_len on {}", tmp.display()))?;

    write_metadata(&mut f, plan)?;

    f.sync_all().context("fsync image")?;
    drop(f);

    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;

    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

fn write_metadata(f: &mut File, plan: &Plan) -> Result<()> {
    write_at(f, 1024, &plan.superblock.to_bytes())?;

    let gdt = gdt_bytes(plan);

    for g in 0..plan.layout.num_groups {
        let gbl = group_block_layout(&plan.layout, g);

        if let Some(sb_block) = gbl.sb_block
            && g != 0
        {
            let backup = plan
                .superblock
                .for_backup_group(u16::try_from(g).expect("group fits u16"));
            write_at(f, sb_block as u64 * BLOCK_SIZE as u64, &backup.to_bytes())?;
        }

        if let Some(gdt_first) = gbl.gdt_first_block {
            write_at(f, gdt_first as u64 * BLOCK_SIZE as u64, &gdt)?;
        }

        let bbmp = block_bitmap(&plan.layout, g);
        write_at(f, gbl.bbmp_block as u64 * BLOCK_SIZE as u64, &bbmp)?;

        let ibmp = inode_bitmap(&plan.layout, g);
        write_at(f, gbl.ibmp_block as u64 * BLOCK_SIZE as u64, &ibmp)?;

        if g == 0 {
            let itab_off = gbl.itab_first_block as u64 * BLOCK_SIZE as u64;
            write_at(
                f,
                itab_off + (ROOT_INO as u64 - 1) * INODE_SIZE as u64,
                &plan.root_inode.to_bytes(),
            )?;
            write_at(
                f,
                itab_off + (LOST_FOUND_INO as u64 - 1) * INODE_SIZE as u64,
                &plan.lost_found_inode.to_bytes(),
            )?;
            write_at(
                f,
                itab_off + (JOURNAL_INO as u64 - 1) * INODE_SIZE as u64,
                &plan.journal_inode.to_bytes(),
            )?;
            if plan.layout.reserved_gdt_blocks > 0 {
                write_at(
                    f,
                    itab_off + (RESIZE_INO as u64 - 1) * INODE_SIZE as u64,
                    &plan.resize_inode.to_bytes(),
                )?;
            }
        }
    }

    write_at(
        f,
        plan.journal_first_block() as u64 * BLOCK_SIZE as u64,
        &journal::superblock_block(plan.journal_blocks(), plan.superblock.uuid),
    )?;

    write_at(
        f,
        plan.root_data_block() as u64 * BLOCK_SIZE as u64,
        &plan.root_dir_block,
    )?;

    for (i, block) in plan.lost_found_blocks.iter().enumerate() {
        let abs = plan.lost_found_first_block() + i as u32;
        write_at(f, abs as u64 * BLOCK_SIZE as u64, block)?;
    }

    if let Some(resize) = resize_blocks(&plan.layout) {
        write_at(
            f,
            resize.dind_block as u64 * BLOCK_SIZE as u64,
            &resize.dind_contents,
        )?;
        for (block, contents) in &resize.indirect {
            write_at(f, *block as u64 * BLOCK_SIZE as u64, contents)?;
        }
    }

    Ok(())
}

fn gdt_bytes(plan: &Plan) -> Vec<u8> {
    let padded_size = plan.layout.gdt_blocks as usize * BLOCK_SIZE as usize;
    let mut out = Vec::with_capacity(padded_size);
    for g in 0..plan.layout.num_groups {
        out.extend_from_slice(&group_descriptor(&plan.layout, g).to_bytes());
    }
    out.resize(padded_size, 0);
    out
}

fn write_at(f: &mut File, offset: u64, data: &[u8]) -> Result<()> {
    f.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seek to {offset:#x}"))?;
    f.write_all(data)
        .with_context(|| format!("write {} bytes at {offset:#x}", data.len()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn build_and_read(image_size: u64) -> (tempfile::TempDir, Plan, Vec<u8>) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(image_size, [0xAA; 16], "lns-upper", 0x12345678).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        let mut buf = Vec::new();
        File::open(&path)
            .expect("open written image")
            .read_to_end(&mut buf)
            .expect("read");
        (dir, plan, buf)
    }

    fn read_u16(buf: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([buf[offset], buf[offset + 1]])
    }
    fn read_u32(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    #[test]
    fn file_size_matches_layout() {
        let (_d, plan, bytes) = build_and_read(32 * 1024 * 1024);
        assert_eq!(bytes.len() as u64, plan.layout.image_size_bytes());
    }

    #[test]
    fn primary_superblock_has_magic_at_canonical_offset() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        assert_eq!(read_u16(&bytes, 0x438), EXT2_SUPER_MAGIC);
    }

    #[test]
    fn primary_superblock_feature_flags_at_canonical_offsets() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        assert_eq!(read_u32(&bytes, 1024 + 0x5C), SB_FEATURE_COMPAT);
        assert_eq!(read_u32(&bytes, 1024 + 0x60), SB_FEATURE_INCOMPAT);
        assert_eq!(read_u32(&bytes, 1024 + 0x64), SB_FEATURE_RO_COMPAT);
    }

    #[test]
    fn boot_sector_is_zero() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        for (i, &b) in bytes[..1024].iter().enumerate() {
            assert_eq!(b, 0, "boot sector byte {i} should be zero");
        }
    }

    #[test]
    fn gdt_at_block_1_for_group_0() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        assert_eq!(read_u32(&bytes, 4096), 9, "bg_block_bitmap");
        assert_eq!(read_u32(&bytes, 4096 + 0x04), 10, "bg_inode_bitmap");
        assert_eq!(read_u32(&bytes, 4096 + 0x08), 11, "bg_inode_table");
        assert_eq!(read_u16(&bytes, 4096 + 0x10), 2);
    }

    #[test]
    fn block_bitmap_for_group_0_marks_every_allocated_block() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let bbmp = &bytes[4096 * 9..4096 * 10];
        for bit in 0..=1168 {
            let set = (bbmp[bit / 8] >> (bit % 8)) & 1 == 1;
            assert!(set, "block bitmap bit {bit} should be set");
        }
        let bit = 1169;
        let set = (bbmp[bit / 8] >> (bit % 8)) & 1 == 1;
        assert!(!set, "bit 1169 should be clear (first free block)");
    }

    #[test]
    fn inode_bitmap_for_group_0_marks_the_reserved_inodes() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let ibmp = &bytes[4096 * 10..4096 * 11];
        assert_eq!(ibmp[0], 0xFF);
        assert_eq!(ibmp[1] & 0b0000_0111, 0b0000_0111);
        assert_eq!(ibmp[1] & 0b0000_1000, 0);
    }

    #[test]
    fn root_inode_at_table_offset_256() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let off = 4096 * 11 + 256;
        assert_eq!(read_u16(&bytes, off), S_IFDIR | 0o755);
        assert_eq!(read_u16(&bytes, off + 0x1A), 3);
        assert_eq!(read_u32(&bytes, off + 0x1C), 8);
        assert_eq!(
            read_u32(&bytes, off + 0x20) & EXT4_EXTENTS_FL,
            EXT4_EXTENTS_FL
        );
        assert_eq!(read_u16(&bytes, off + 0x28), EXT4_EXTENT_MAGIC);
        assert_eq!(read_u16(&bytes, off + 0x2A), 1);
        assert_eq!(read_u32(&bytes, off + 0x34), 0);
        assert_eq!(read_u16(&bytes, off + 0x38), 1);
        assert_eq!(read_u32(&bytes, off + 0x3C), 139);
    }

    #[test]
    fn lost_found_inode_at_table_offset_2560() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let off = 4096 * 11 + 2560;
        assert_eq!(read_u16(&bytes, off), S_IFDIR | 0o700);
        assert_eq!(read_u16(&bytes, off + 0x1A), 2);
        assert_eq!(read_u32(&bytes, off + 0x1C), 8 * LOST_FOUND_BLOCKS);
        assert_eq!(
            read_u32(&bytes, off + 0x20) & EXT4_EXTENTS_FL,
            EXT4_EXTENTS_FL
        );
        assert_eq!(read_u16(&bytes, off + 0x28), EXT4_EXTENT_MAGIC);
        assert_eq!(read_u16(&bytes, off + 0x2A), 1, "one extent of length 4");
        assert_eq!(read_u32(&bytes, off + 0x34), 0, "ee_block");
        assert_eq!(read_u16(&bytes, off + 0x38), 4, "ee_len");
        assert_eq!(read_u32(&bytes, off + 0x3C), 140, "ee_start_lo");
    }

    fn inode_table_offset(plan: &Plan) -> usize {
        group_block_layout(&plan.layout, 0).itab_first_block as usize * BLOCK_SIZE as usize
    }

    #[test]
    fn the_written_image_contains_the_journal_inode_in_group_zero() {
        let (_d, plan, bytes) = build_and_read(32 * 1024 * 1024);
        let off = inode_table_offset(&plan) + (JOURNAL_INO as usize - 1) * INODE_SIZE as usize;
        assert_eq!(
            &bytes[off..off + INODE_SIZE as usize],
            &plan.journal_inode.to_bytes()
        );
    }

    #[test]
    fn the_written_image_starts_the_journal_with_a_valid_jbd2_superblock() {
        let (_d, plan, bytes) = build_and_read(32 * 1024 * 1024);
        let off = plan.journal_first_block() as usize * BLOCK_SIZE as usize;
        assert_eq!(&bytes[off..off + 4], &JBD2_MAGIC.to_be_bytes());
        assert_eq!(
            u32::from_be_bytes([
                bytes[off + 0x10],
                bytes[off + 0x11],
                bytes[off + 0x12],
                bytes[off + 0x13]
            ]),
            plan.journal_blocks(),
            "s_maxlen must match the extent the inode reserved"
        );
    }

    #[test]
    fn the_journal_flag_and_the_jbd2_block_come_from_one_plan() {
        let (_d, plan, bytes) = build_and_read(32 * 1024 * 1024);
        assert_eq!(
            read_u32(&bytes, 1024 + 0x5C) & FEATURE_COMPAT_HAS_JOURNAL,
            FEATURE_COMPAT_HAS_JOURNAL,
            "a flag with no journal behind it makes the kernel refuse the rw mount outright"
        );
        let inum = read_u32(&bytes, 1024 + 0xE0);
        let inode_off = inode_table_offset(&plan) + (inum as usize - 1) * INODE_SIZE as usize;
        let first_block = read_u32(&bytes, inode_off + 0x3C) as usize;
        assert_eq!(
            &bytes[first_block * BLOCK_SIZE as usize..first_block * BLOCK_SIZE as usize + 4],
            &JBD2_MAGIC.to_be_bytes(),
            "the block the advertised inode points at must hold the JBD2 superblock"
        );
    }

    #[test]
    fn root_dir_block_byte_layout() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let off = 4096 * 139;
        assert_eq!(read_u32(&bytes, off), ROOT_INO);
        assert_eq!(read_u16(&bytes, off + 4), 12);
        assert_eq!(bytes[off + 6], 1);
        assert_eq!(&bytes[off + 8..off + 9], b".");
        assert_eq!(read_u32(&bytes, off + 12), ROOT_INO);
        assert_eq!(read_u16(&bytes, off + 16), 12);
        assert_eq!(read_u32(&bytes, off + 24), LOST_FOUND_INO);
        assert_eq!(read_u16(&bytes, off + 28), 4072);
        assert_eq!(&bytes[off + 32..off + 42], b"lost+found");
    }

    #[test]
    fn lost_found_first_block_byte_layout() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        let off = 4096 * 140;
        assert_eq!(read_u32(&bytes, off), LOST_FOUND_INO);
        assert_eq!(read_u16(&bytes, off + 4), 12);
        assert_eq!(read_u32(&bytes, off + 12), ROOT_INO);
        assert_eq!(read_u16(&bytes, off + 16), 4084);
    }

    #[test]
    fn lost_found_trailing_blocks_are_sentinels() {
        let (_d, _p, bytes) = build_and_read(32 * 1024 * 1024);
        for i in 1..LOST_FOUND_BLOCKS as usize {
            let off = 4096 * (140 + i);
            assert_eq!(read_u32(&bytes, off), 0);
            assert_eq!(read_u16(&bytes, off + 4), BLOCK_SIZE as u16);
        }
    }

    #[test]
    fn backup_superblock_has_correct_block_group_nr() {
        let (_d, _p, bytes) = build_and_read(256 * 1024 * 1024);
        let backup_off = 32768usize * 4096;
        assert_eq!(read_u16(&bytes, backup_off + 0x38), EXT2_SUPER_MAGIC);
        assert_eq!(read_u16(&bytes, backup_off + 0x5A), 1);
        assert_eq!(read_u32(&bytes, backup_off + 0x5C), SB_FEATURE_COMPAT);
    }

    #[test]
    fn no_tmp_file_left_after_success() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "test", 0).expect("plan");
        write_ext4(&plan, &path).expect("write");
        assert!(path.exists(), "image exists");
        let tmp = tmp_path(&path);
        assert!(!tmp.exists(), "tmp must be cleaned up after rename");
    }

    #[test]
    fn stale_tmp_is_overwritten() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let tmp = tmp_path(&path);
        std::fs::write(&tmp, b"stale junk").expect("seed stale tmp");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "test", 0).expect("plan");
        write_ext4(&plan, &path).expect("write should succeed despite stale tmp");
        assert!(path.exists());
        assert!(!tmp.exists());
    }
}
