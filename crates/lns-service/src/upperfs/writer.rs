use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::upperfs::constants::*;
use crate::upperfs::format::Superblock;
use crate::upperfs::grow::GrowPlan;
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

/// Grows an image in place towards `new_size_bytes`, keeping every block already written; a live image is far too large to copy, so the write order is the safety and the primary superblock is the commit.
pub fn grow_ext4(path: &Path, new_size_bytes: u64) -> Result<()> {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;

    let mut sb_bytes = [0u8; 1024];
    read_at(&mut f, 1024, &mut sb_bytes).with_context(|| format!("reading {}", path.display()))?;
    let superblock = Superblock::from_bytes(&sb_bytes)
        .with_context(|| format!("reading the superblock of {}", path.display()))?;

    let Some(plan) = crate::upperfs::grow::plan_grow(&superblock, new_size_bytes)? else {
        return Ok(());
    };

    mark_unclean(&mut f, &superblock)?;

    f.set_len(plan.image_size_bytes())
        .with_context(|| format!("set_len on {}", path.display()))?;
    f.sync_all().context("fsync after extending the image")?;

    write_appended_groups(&mut f, &plan)?;
    f.sync_all().context("fsync appended groups")?;

    let dind_block = read_resize_dind_block(&mut f, &plan.before)?;

    let mut live_gdt = vec![0u8; plan.before.gdt_blocks as usize * BLOCK_SIZE as usize];
    read_at(&mut f, gdt_offset(&plan.before, 0), &mut live_gdt)?;
    let mut gdt = grown_gdt_bytes(&plan, &live_gdt);
    retally_filled_tail(&plan, &mut gdt);
    write_at(&mut f, gdt_offset(&plan.after, 0), &gdt)?;
    fill_old_tail(&mut f, &plan)?;
    write_resize_tree(&mut f, &plan, dind_block)?;
    f.sync_all().context("fsync the descriptor table")?;

    refresh_backups(&mut f, &plan, &gdt)?;
    f.sync_all().context("fsync the backup superblocks")?;

    write_at(&mut f, 1024, &commit_superblock(&plan).to_bytes())?;
    f.sync_all().context("fsync the superblock")?;

    Ok(())
}

/// A grow that stops halfway leaves a valid smaller filesystem inside a longer file, and this bit is what tells the kernel and e2fsck to reconcile the two rather than trust the counts.
fn mark_unclean(f: &mut File, superblock: &Superblock) -> Result<()> {
    let mut unclean = superblock.clone();
    unclean.state = 0;
    write_at(f, 1024, &unclean.to_bytes())?;
    f.sync_all().context("fsync the unclean marker")?;
    Ok(())
}

fn commit_superblock(plan: &GrowPlan) -> Superblock {
    let mut sb = plan.superblock.clone();
    sb.state = EXT2_VALID_FS;
    sb.free_blocks_count = total_free_blocks(plan);
    sb.free_inodes_count =
        plan.superblock.free_inodes_count + (plan.after.inodes_count - plan.before.inodes_count);
    sb
}

fn total_free_blocks(plan: &GrowPlan) -> u32 {
    let appended: u32 = plan
        .appended_groups
        .clone()
        .map(|g| u32::from(group_descriptor(&plan.after, g).free_blocks_count))
        .sum();
    let tail_gain = plan
        .filled_tail
        .as_ref()
        .map(|(_, gained)| gained.end - gained.start)
        .unwrap_or(0);
    plan.superblock.free_blocks_count + appended + tail_gain
}

fn gdt_offset(layout: &crate::upperfs::layout::Layout, g: u32) -> u64 {
    let gbl = group_block_layout(layout, g);
    gbl.gdt_first_block
        .expect("only a group carrying a backup is asked for its table") as u64
        * BLOCK_SIZE as u64
}

/// Descriptors for groups that already existed are copied from the live table, never recomputed: they carry allocation counts this writer has no other record of.
fn grown_gdt_bytes(plan: &GrowPlan, live_gdt: &[u8]) -> Vec<u8> {
    let padded = plan.after.gdt_blocks as usize * BLOCK_SIZE as usize;
    let desc = GROUP_DESC_SIZE as usize;
    let mut out = Vec::with_capacity(padded);
    for g in 0..plan.after.num_groups {
        if plan.appended_groups.contains(&g) {
            out.extend_from_slice(&group_descriptor(&plan.after, g).to_bytes());
        } else {
            let at = g as usize * desc;
            out.extend_from_slice(&live_gdt[at..at + desc]);
        }
    }
    out.resize(padded, 0);
    out
}

/// The tail group's descriptor is the one existing entry a grow rewrites, because the blocks it just gained are free now.
fn retally_filled_tail(plan: &GrowPlan, gdt: &mut [u8]) {
    let Some((tail, gained)) = &plan.filled_tail else {
        return;
    };
    let at = *tail as usize * GROUP_DESC_SIZE as usize;
    let was = u16::from_le_bytes([gdt[at + 0x0C], gdt[at + 0x0D]]);
    let now = was + u16::try_from(gained.end - gained.start).expect("a group's blocks fit u16");
    gdt[at + 0x0C..at + 0x0E].copy_from_slice(&now.to_le_bytes());
}

fn write_appended_groups(f: &mut File, plan: &GrowPlan) -> Result<()> {
    for g in plan.appended_groups.clone() {
        let gbl = group_block_layout(&plan.after, g);
        write_at(
            f,
            gbl.bbmp_block as u64 * BLOCK_SIZE as u64,
            &block_bitmap(&plan.after, g),
        )?;
        write_at(
            f,
            gbl.ibmp_block as u64 * BLOCK_SIZE as u64,
            &inode_bitmap(&plan.after, g),
        )?;
    }
    Ok(())
}

/// The one existing group a grow edits. Its padding bits stood for blocks the image did not have; now it does, so they become free.
fn fill_old_tail(f: &mut File, plan: &GrowPlan) -> Result<()> {
    let Some((tail, _)) = &plan.filled_tail else {
        return Ok(());
    };
    let gbl = group_block_layout(&plan.after, *tail);
    let offset = gbl.bbmp_block as u64 * BLOCK_SIZE as u64;
    let mut bitmap = vec![0u8; BLOCK_SIZE as usize];
    read_at(f, offset, &mut bitmap)?;
    for bit in plan.filled_tail.as_ref().expect("checked above").1.clone() {
        clear_bit(&mut bitmap, bit as usize);
    }
    write_at(f, offset, &bitmap)?;
    Ok(())
}

fn write_resize_tree(f: &mut File, plan: &GrowPlan, dind_block: u32) -> Result<()> {
    let Some(resize) = resize_blocks(&plan.after, dind_block) else {
        return Ok(());
    };
    write_at(
        f,
        resize.dind_block as u64 * BLOCK_SIZE as u64,
        &resize.dind_contents,
    )?;
    for (block, contents) in &resize.indirect {
        write_at(f, *block as u64 * BLOCK_SIZE as u64, contents)?;
    }
    let gbl = group_block_layout(&plan.after, 0);
    let inode = crate::upperfs::format::Inode::resize(
        plan.superblock.wtime,
        resize.dind_block,
        plan.after.reserved_gdt_blocks,
        plan.after.backup_groups.iter().filter(|g| **g > 0).count() as u32,
    );
    write_at(
        f,
        gbl.itab_first_block as u64 * BLOCK_SIZE as u64
            + (RESIZE_INO as u64 - 1) * INODE_SIZE as u64,
        &inode.to_bytes(),
    )
}

fn refresh_backups(f: &mut File, plan: &GrowPlan, gdt: &[u8]) -> Result<()> {
    let committed = commit_superblock(plan);
    for g in plan.after.backup_groups.iter().copied().filter(|g| *g > 0) {
        let gbl = group_block_layout(&plan.after, g);
        let sb = committed.for_backup_group(u16::try_from(g).expect("group fits u16"));
        write_at(
            f,
            gbl.sb_block.expect("a backup group carries a superblock") as u64 * BLOCK_SIZE as u64,
            &sb.to_bytes(),
        )?;
        write_at(f, gdt_offset(&plan.after, g), gdt)?;
    }
    Ok(())
}

/// The image already decided where its resize block lives, and that position follows the journal size it was created with — recomputing it from the new size moves it.
fn read_resize_dind_block(f: &mut File, before: &crate::upperfs::layout::Layout) -> Result<u32> {
    let gbl = group_block_layout(before, 0);
    let mut inode = [0u8; INODE_SIZE as usize];
    read_at(
        f,
        gbl.itab_first_block as u64 * BLOCK_SIZE as u64
            + (RESIZE_INO as u64 - 1) * INODE_SIZE as u64,
        &mut inode,
    )?;
    let at = 0x28 + EXT2_DIND_BLOCK * 4;
    Ok(u32::from_le_bytes([
        inode[at],
        inode[at + 1],
        inode[at + 2],
        inode[at + 3],
    ]))
}

fn read_at(f: &mut File, offset: u64, buf: &mut [u8]) -> Result<()> {
    f.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seek to {offset:#x}"))?;
    f.read_exact(buf)
        .with_context(|| format!("read {} bytes at {offset:#x}", buf.len()))?;
    Ok(())
}

fn clear_bit(buf: &mut [u8], bit: usize) {
    buf[bit / 8] &= !(1u8 << (bit % 8));
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

    if let Some(resize) = resize_blocks(
        &plan.layout,
        crate::upperfs::plan::resize_dind_block(&plan.layout),
    ) {
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

    fn grow_and_read(from: u64, to: u64) -> (tempfile::TempDir, Plan, Vec<u8>) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(from, [0xAA; 16], "lns-upper", 0x12345678).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        grow_ext4(&path, to).expect("grow_ext4");
        let mut buf = Vec::new();
        File::open(&path)
            .expect("open grown image")
            .read_to_end(&mut buf)
            .expect("read");
        (dir, plan, buf)
    }

    fn superblock_of(bytes: &[u8]) -> Superblock {
        let mut raw = [0u8; 1024];
        raw.copy_from_slice(&bytes[1024..2048]);
        Superblock::from_bytes(&raw).expect("a written image reads back")
    }

    #[test]
    fn a_grow_publishes_the_new_size_and_leaves_the_image_clean() {
        let (_d, plan, bytes) = grow_and_read(130 * 1024 * 1024, 400 * 1024 * 1024);
        let sb = superblock_of(&bytes);
        assert_eq!(sb.blocks_count, 102_400);
        assert_eq!(
            sb.state, EXT2_VALID_FS,
            "an image left unclean would be repaired on the next mount"
        );
        assert_eq!(bytes.len() as u64, 400 * 1024 * 1024);
        assert_eq!(
            sb.inodes_per_group, plan.superblock.inodes_per_group,
            "the density the image was created with is the density it keeps"
        );
    }

    /// The resize block sits after the journal, and the journal's size follows the size the image was created at — so recomputing its position from the new size moves it and orphans a block.
    #[test]
    fn a_grow_leaves_the_resize_block_where_the_image_put_it() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        let before = read_resize_dind_block(
            &mut File::options().read(true).write(true).open(&path).unwrap(),
            &plan.layout,
        )
        .expect("read the resize block");

        grow_ext4(&path, 160 * 1024 * 1024).expect("grow_ext4");

        let after = read_resize_dind_block(
            &mut File::options().read(true).write(true).open(&path).unwrap(),
            &plan.layout,
        )
        .expect("read the resize block");
        assert_eq!(
            after, before,
            "a grow that moves this block leaves the old one allocated and the new one unaccounted for"
        );
    }

    #[test]
    fn a_grow_that_asks_for_no_more_than_the_image_has_writes_nothing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        let before = std::fs::read(&path).expect("read");

        grow_ext4(&path, 32 * 1024 * 1024).expect("grow_ext4");
        grow_ext4(&path, 16 * 1024 * 1024).expect("grow_ext4");

        assert_eq!(
            std::fs::read(&path).expect("read"),
            before,
            "an image that already meets the request must not be touched at all"
        );
    }

    /// A grow onto a whole-group boundary edits no existing group, so both tail paths must fall straight through.
    #[test]
    fn a_grow_onto_a_group_boundary_touches_no_existing_group() {
        let (_d, _p, bytes) = grow_and_read(256 * 1024 * 1024, 384 * 1024 * 1024);
        let sb = superblock_of(&bytes);
        assert_eq!(sb.blocks_count, 98_304, "three whole groups");
        assert_eq!(sb.state, EXT2_VALID_FS);
    }

    /// An image written before volumes reserved room to grow carries no resize tree, so a grow inside its existing table must leave that tree alone rather than invent one.
    #[test]
    fn a_grow_of_an_image_with_no_reserved_run_writes_no_resize_tree() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");

        let mut stripped = plan.superblock.clone();
        stripped.reserved_gdt_blocks = 0;
        stripped.feature_compat &= !FEATURE_COMPAT_RESIZE_INODE;
        let mut f = File::options().read(true).write(true).open(&path).unwrap();
        write_at(&mut f, 1024, &stripped.to_bytes()).expect("strip the reserved run");
        drop(f);

        grow_ext4(&path, 64 * 1024 * 1024).expect("grow_ext4");

        let bytes = std::fs::read(&path).expect("read");
        let sb = superblock_of(&bytes);
        assert_eq!(sb.blocks_count, 16_384, "the image still grew");
        assert_eq!(
            sb.reserved_gdt_blocks, 0,
            "an image with nothing reserved must not gain a reservation it never had room for"
        );
    }

    #[test]
    fn a_grow_refuses_an_image_that_was_not_unmounted_cleanly() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");

        let mut unclean = plan.superblock.clone();
        unclean.state = 0;
        let mut f = File::options().read(true).write(true).open(&path).unwrap();
        write_at(&mut f, 1024, &unclean.to_bytes()).expect("mark unclean");
        drop(f);
        let before = std::fs::read(&path).expect("read");

        let err = grow_ext4(&path, 160 * 1024 * 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("not unmounted cleanly"),
            "growing a dirty image would erase the record of what the crash left unfinished: {err:#}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read"),
            before,
            "a refused grow must leave the volume exactly as it was"
        );
    }

    #[test]
    fn a_grow_refuses_a_superblock_whose_counts_contradict_each_other() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");

        let mut empty = plan.superblock.clone();
        empty.blocks_count = 0;
        empty.inodes_count = 0;
        let mut f = File::options().read(true).write(true).open(&path).unwrap();
        write_at(&mut f, 1024, &empty.to_bytes()).expect("write");
        drop(f);

        let err = grow_ext4(&path, 160 * 1024 * 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("no blocks at all"),
            "a count nothing can be derived from has to refuse, not wrap around: {err:#}"
        );
    }

    #[test]
    fn a_grow_refuses_an_image_this_writer_did_not_produce() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("not-ext4.img");
        std::fs::write(&path, vec![0u8; 64 * 1024]).expect("write");
        let err = grow_ext4(&path, 128 * 1024 * 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("not an ext4 image"),
            "growing a file that is not one of our images must refuse, not rewrite it: {err:#}"
        );
    }

    #[test]
    fn a_grow_past_the_reserved_run_refuses_before_it_writes_anything() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0; 16], "t", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        let before = std::fs::read(&path).expect("read");

        let err = grow_ext4(&path, 64 * 1024 * 1024 * 1024 * 1024).unwrap_err();
        assert!(
            format!("{err:#}").contains("can grow to at most"),
            "{err:#}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read"),
            before,
            "a refused grow must leave the volume exactly as it was"
        );
    }

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
