use crate::upperfs::constants::*;
use crate::upperfs::extents::{ExtentLeaf, I_BLOCK_BYTES, encode_inline_leaf};
use crate::upperfs::layout::Layout;

#[derive(Debug, Clone)]
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count: u32,
    pub r_blocks_count: u32,
    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    pub first_data_block: u32,
    pub log_block_size: u32,
    // log_cluster_size MUST mirror log_block_size when BIGALLOC is clear — e2fsprogs treats a mismatch as fatal.
    pub log_cluster_size: u32,
    pub blocks_per_group: u32,
    pub clusters_per_group: u32,
    pub inodes_per_group: u32,
    pub mtime: u32,
    pub wtime: u32,
    pub mnt_count: u16,
    pub max_mnt_count: i16,
    pub magic: u16,
    pub state: u16,
    pub errors: u16,
    pub minor_rev_level: u16,
    pub lastcheck: u32,
    pub checkinterval: u32,
    pub creator_os: u32,
    pub rev_level: u32,
    pub def_resuid: u16,
    pub def_resgid: u16,
    pub first_ino: u32,
    pub inode_size: u16,
    pub block_group_nr: u16,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
    // reserved_gdt_blocks MUST be 0 when COMPAT_RESIZE_INODE is clear — e2fsck refuses any non-zero value without the matching feature.
    pub reserved_gdt_blocks: u16,
    pub journal_inum: u32,
    pub mkfs_time: u32,
}

impl Superblock {
    pub fn for_fresh_image(
        layout: &Layout,
        uuid: [u8; 16],
        volume_name: &str,
        mkfs_time: u32,
    ) -> Self {
        Self {
            inodes_count: layout.inodes_count,
            blocks_count: layout.block_count,
            r_blocks_count: 0,
            free_blocks_count: layout.block_count,
            free_inodes_count: layout.inodes_count,
            first_data_block: 0,
            log_block_size: LOG_BLOCK_SIZE,
            log_cluster_size: LOG_BLOCK_SIZE,
            blocks_per_group: layout.blocks_per_group,
            clusters_per_group: layout.blocks_per_group,
            inodes_per_group: layout.inodes_per_group,
            mtime: 0,
            wtime: mkfs_time,
            mnt_count: 0,
            max_mnt_count: -1,
            magic: EXT2_SUPER_MAGIC,
            state: EXT2_VALID_FS,
            errors: EXT2_ERRORS_CONTINUE,
            minor_rev_level: 0,
            lastcheck: mkfs_time,
            checkinterval: 0,
            creator_os: EXT2_OS_LINUX,
            rev_level: EXT2_DYNAMIC_REV,
            def_resuid: 0,
            def_resgid: 0,
            first_ino: FIRST_INO,
            inode_size: INODE_SIZE,
            block_group_nr: 0,
            feature_compat: if layout.reserved_gdt_blocks > 0 {
                SB_FEATURE_COMPAT
            } else {
                SB_FEATURE_COMPAT & !FEATURE_COMPAT_RESIZE_INODE
            },
            feature_incompat: SB_FEATURE_INCOMPAT,
            feature_ro_compat: SB_FEATURE_RO_COMPAT,
            uuid,
            volume_name: pad_name::<16>(volume_name),
            reserved_gdt_blocks: u16::try_from(layout.reserved_gdt_blocks)
                .expect("reserved_gdt_blocks is capped at one block of addresses"),
            journal_inum: JOURNAL_INO,
            mkfs_time,
        }
    }

    pub fn to_bytes(&self) -> [u8; 1024] {
        let mut b = [0u8; 1024];
        b[0x00..0x04].copy_from_slice(&self.inodes_count.to_le_bytes());
        b[0x04..0x08].copy_from_slice(&self.blocks_count.to_le_bytes());
        b[0x08..0x0C].copy_from_slice(&self.r_blocks_count.to_le_bytes());
        b[0x0C..0x10].copy_from_slice(&self.free_blocks_count.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.free_inodes_count.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&self.first_data_block.to_le_bytes());
        b[0x18..0x1C].copy_from_slice(&self.log_block_size.to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&self.log_cluster_size.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&self.blocks_per_group.to_le_bytes());
        b[0x24..0x28].copy_from_slice(&self.clusters_per_group.to_le_bytes());
        b[0x28..0x2C].copy_from_slice(&self.inodes_per_group.to_le_bytes());
        b[0x2C..0x30].copy_from_slice(&self.mtime.to_le_bytes());
        b[0x30..0x34].copy_from_slice(&self.wtime.to_le_bytes());
        b[0x34..0x36].copy_from_slice(&self.mnt_count.to_le_bytes());
        b[0x36..0x38].copy_from_slice(&self.max_mnt_count.to_le_bytes());
        b[0x38..0x3A].copy_from_slice(&self.magic.to_le_bytes());
        b[0x3A..0x3C].copy_from_slice(&self.state.to_le_bytes());
        b[0x3C..0x3E].copy_from_slice(&self.errors.to_le_bytes());
        b[0x3E..0x40].copy_from_slice(&self.minor_rev_level.to_le_bytes());
        b[0x40..0x44].copy_from_slice(&self.lastcheck.to_le_bytes());
        b[0x44..0x48].copy_from_slice(&self.checkinterval.to_le_bytes());
        b[0x48..0x4C].copy_from_slice(&self.creator_os.to_le_bytes());
        b[0x4C..0x50].copy_from_slice(&self.rev_level.to_le_bytes());
        b[0x50..0x52].copy_from_slice(&self.def_resuid.to_le_bytes());
        b[0x52..0x54].copy_from_slice(&self.def_resgid.to_le_bytes());
        b[0x54..0x58].copy_from_slice(&self.first_ino.to_le_bytes());
        b[0x58..0x5A].copy_from_slice(&self.inode_size.to_le_bytes());
        b[0x5A..0x5C].copy_from_slice(&self.block_group_nr.to_le_bytes());
        b[0x5C..0x60].copy_from_slice(&self.feature_compat.to_le_bytes());
        b[0x60..0x64].copy_from_slice(&self.feature_incompat.to_le_bytes());
        b[0x64..0x68].copy_from_slice(&self.feature_ro_compat.to_le_bytes());
        b[0x68..0x78].copy_from_slice(&self.uuid);
        b[0x78..0x88].copy_from_slice(&self.volume_name);
        b[0xCE..0xD0].copy_from_slice(&self.reserved_gdt_blocks.to_le_bytes());
        b[0xE0..0xE4].copy_from_slice(&self.journal_inum.to_le_bytes());
        // 0xD0..0xE0 (s_journal_uuid) and 0xE4..0xE8 (s_journal_dev) MUST stay zero — a non-zero value there means the journal lives on an external device.
        // 0xFE..0x100: s_desc_size MUST be 0 when INCOMPAT_64BIT is clear — e2fsprogs treats non-zero as "64bit implied" and errors.
        b[0x108..0x10C].copy_from_slice(&self.mkfs_time.to_le_bytes());
        b
    }

    /// Reads back what a written image says about itself, saying nothing about whether it may be written to.
    pub fn from_bytes(b: &[u8; 1024]) -> anyhow::Result<Self> {
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

        let magic = u16_at(0x38);
        anyhow::ensure!(
            magic == EXT2_SUPER_MAGIC,
            "not an ext4 image: superblock magic is {magic:#06x}, not {EXT2_SUPER_MAGIC:#06x}"
        );
        let log_block_size = u32_at(0x18);
        anyhow::ensure!(
            log_block_size == LOG_BLOCK_SIZE,
            "image uses {}-byte blocks; this writer only handles {BLOCK_SIZE}",
            1024u32 << log_block_size
        );
        let blocks_per_group = u32_at(0x20);
        anyhow::ensure!(
            blocks_per_group == BLOCKS_PER_GROUP,
            "image puts {blocks_per_group} blocks in a group; this writer only handles {BLOCKS_PER_GROUP}"
        );
        let inode_size = u16_at(0x58);
        anyhow::ensure!(
            inode_size == INODE_SIZE,
            "image uses {inode_size}-byte inodes; this writer only handles {INODE_SIZE}"
        );

        let feature_incompat = u32_at(0x60);
        let feature_ro_compat = u32_at(0x64);
        let state = u16_at(0x3A);

        let inodes_count = u32_at(0x00);
        let blocks_count = u32_at(0x04);
        let inodes_per_group = u32_at(0x28);
        anyhow::ensure!(blocks_count > 0, "image counts no blocks at all");
        let num_groups = blocks_count.div_ceil(blocks_per_group);
        anyhow::ensure!(
            inodes_per_group != 0 && inodes_per_group.checked_mul(num_groups) == Some(inodes_count),
            "image counts {inodes_count} inodes but {inodes_per_group} per group across {num_groups} groups"
        );

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&b[0x68..0x78]);
        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&b[0x78..0x88]);

        Ok(Self {
            inodes_count,
            blocks_count,
            r_blocks_count: u32_at(0x08),
            free_blocks_count: u32_at(0x0C),
            free_inodes_count: u32_at(0x10),
            first_data_block: u32_at(0x14),
            log_block_size,
            log_cluster_size: u32_at(0x1C),
            blocks_per_group,
            clusters_per_group: u32_at(0x24),
            inodes_per_group,
            mtime: u32_at(0x2C),
            wtime: u32_at(0x30),
            mnt_count: u16_at(0x34),
            max_mnt_count: u16_at(0x36) as i16,
            magic,
            state,
            errors: u16_at(0x3C),
            minor_rev_level: u16_at(0x3E),
            lastcheck: u32_at(0x40),
            checkinterval: u32_at(0x44),
            creator_os: u32_at(0x48),
            rev_level: u32_at(0x4C),
            def_resuid: u16_at(0x50),
            def_resgid: u16_at(0x52),
            first_ino: u32_at(0x54),
            inode_size,
            block_group_nr: u16_at(0x5A),
            feature_compat: u32_at(0x5C),
            feature_incompat,
            feature_ro_compat,
            uuid,
            volume_name,
            reserved_gdt_blocks: u16_at(0xCE),
            journal_inum: u32_at(0xE0),
            mkfs_time: u32_at(0x108),
        })
    }

    /// The features an in-place rewrite must be able to reproduce, checked whatever else is known about the image — `INCOMPAT_RECOVER` is one of them, so a guest's unreplayed journal is refused here.
    pub fn ensure_rewritable_features(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.feature_incompat & FEATURE_INCOMPAT_RECOVER == 0,
            "the volume's journal still holds writes this resize would overwrite; run the sandbox once to let the guest replay it, then ask for the larger size again"
        );
        let incompat = self.feature_incompat & !SB_FEATURE_INCOMPAT;
        anyhow::ensure!(
            incompat == 0,
            "the volume sets features {incompat:#010x} this writer cannot rewrite"
        );
        let ro_compat = self.feature_ro_compat & !SB_FEATURE_RO_COMPAT;
        anyhow::ensure!(
            ro_compat == 0,
            "the volume sets read-only features {ro_compat:#010x} this writer cannot rewrite"
        );
        Ok(())
    }

    /// Whether the filesystem is settled enough to rewrite; only a writer needs this, so it is checked when there is something to write and not before.
    pub fn ensure_unmounted_cleanly(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.state == EXT2_VALID_FS,
            "the volume was not unmounted cleanly, so its journal still holds writes this resize would overwrite; run the sandbox once to let the guest replay it, then ask for the larger size again"
        );
        Ok(())
    }

    pub fn for_backup_group(&self, g: u16) -> Self {
        let mut s = self.clone();
        s.block_group_nr = g;
        s
    }
}

fn pad_name<const N: usize>(s: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    let n = bytes.len().min(N);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

#[derive(Debug, Clone, Default)]
pub struct GroupDescriptor {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
    pub free_blocks_count: u16,
    pub free_inodes_count: u16,
    pub used_dirs_count: u16,
    pub flags: u16,
    pub itable_unused: u16,
}

impl GroupDescriptor {
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0x00..0x04].copy_from_slice(&self.block_bitmap.to_le_bytes());
        b[0x04..0x08].copy_from_slice(&self.inode_bitmap.to_le_bytes());
        b[0x08..0x0C].copy_from_slice(&self.inode_table.to_le_bytes());
        b[0x0C..0x0E].copy_from_slice(&self.free_blocks_count.to_le_bytes());
        b[0x0E..0x10].copy_from_slice(&self.free_inodes_count.to_le_bytes());
        b[0x10..0x12].copy_from_slice(&self.used_dirs_count.to_le_bytes());
        b[0x12..0x14].copy_from_slice(&self.flags.to_le_bytes());
        b[0x1C..0x1E].copy_from_slice(&self.itable_unused.to_le_bytes());
        b
    }
}

#[derive(Debug, Clone)]
pub struct Inode {
    pub mode: u16,
    pub uid: u16,
    pub size: u32,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub gid: u16,
    pub links_count: u16,
    pub blocks: u32,
    pub flags: u32,
    pub i_block: [u8; I_BLOCK_BYTES],
    pub generation: u32,
    pub file_acl: u32,
    pub size_high: u32,
}

impl Default for Inode {
    fn default() -> Self {
        Self {
            mode: 0,
            uid: 0,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            links_count: 0,
            blocks: 0,
            flags: 0,
            i_block: [0u8; I_BLOCK_BYTES],
            generation: 0,
            file_acl: 0,
            size_high: 0,
        }
    }
}

impl Inode {
    pub fn root_directory(time: u32, root_data_block: u32) -> Self {
        let i_block = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 1,
            physical_block: root_data_block as u64,
        }]);
        Self {
            mode: S_IFDIR | 0o755,
            uid: 0,
            size: BLOCK_SIZE,
            atime: time,
            ctime: time,
            mtime: time,
            dtime: 0,
            gid: 0,
            links_count: 3,
            blocks: BLOCK_SIZE / 512,
            flags: EXT4_EXTENTS_FL,
            i_block,
            generation: 0,
            file_acl: 0,
            size_high: 0,
        }
    }

    pub fn lost_found_directory(time: u32, first_data_block: u32) -> Self {
        let i_block = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: u16::try_from(LOST_FOUND_BLOCKS).expect("LOST_FOUND_BLOCKS fits u16"),
            physical_block: first_data_block as u64,
        }]);
        Self {
            mode: S_IFDIR | 0o700,
            uid: 0,
            size: BLOCK_SIZE * LOST_FOUND_BLOCKS,
            atime: time,
            ctime: time,
            mtime: time,
            dtime: 0,
            gid: 0,
            links_count: 2,
            blocks: (BLOCK_SIZE / 512) * LOST_FOUND_BLOCKS,
            flags: EXT4_EXTENTS_FL,
            i_block,
            generation: 0,
            file_acl: 0,
            size_high: 0,
        }
    }

    pub fn journal(time: u32, first_block: u32, journal_blocks: u32) -> Self {
        let i_block = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: u16::try_from(journal_blocks).expect("journal_blocks fits one extent record"),
            physical_block: first_block as u64,
        }]);
        Self {
            mode: S_IFREG | 0o600,
            uid: 0,
            size: journal_blocks * BLOCK_SIZE,
            atime: time,
            ctime: time,
            mtime: time,
            dtime: 0,
            gid: 0,
            links_count: 1,
            blocks: journal_blocks * (BLOCK_SIZE / 512),
            flags: EXT4_EXTENTS_FL,
            i_block,
            generation: 0,
            file_acl: 0,
            size_high: 0,
        }
    }

    /// Inode 7, which owns the reserved GDT blocks so a later grow can extend the descriptor table into them.
    pub fn resize(
        time: u32,
        dind_block: u32,
        reserved_gdt_blocks: u32,
        backup_groups: u32,
    ) -> Self {
        let mut i_block = [0u8; I_BLOCK_BYTES];
        i_block[EXT2_DIND_BLOCK * 4..EXT2_DIND_BLOCK * 4 + 4]
            .copy_from_slice(&dind_block.to_le_bytes());

        let visited = 1 + reserved_gdt_blocks + reserved_gdt_blocks * backup_groups;
        let size = u64::from(ADDRS_PER_BLOCK) * u64::from(ADDRS_PER_BLOCK)
            + u64::from(ADDRS_PER_BLOCK)
            + u64::from(EXT2_NDIR_BLOCKS);
        let size = size * u64::from(BLOCK_SIZE);

        Self {
            mode: S_IFREG | 0o600,
            uid: 0,
            size: size as u32,
            atime: time,
            ctime: time,
            mtime: time,
            dtime: 0,
            gid: 0,
            links_count: 1,
            blocks: visited * (BLOCK_SIZE / 512),
            flags: 0,
            i_block,
            generation: 0,
            file_acl: 0,
            size_high: (size >> 32) as u32,
        }
    }

    pub fn to_bytes(&self) -> [u8; INODE_SIZE as usize] {
        let mut b = [0u8; INODE_SIZE as usize];
        b[0x00..0x02].copy_from_slice(&self.mode.to_le_bytes());
        b[0x02..0x04].copy_from_slice(&self.uid.to_le_bytes());
        b[0x04..0x08].copy_from_slice(&self.size.to_le_bytes());
        b[0x08..0x0C].copy_from_slice(&self.atime.to_le_bytes());
        b[0x0C..0x10].copy_from_slice(&self.ctime.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.mtime.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&self.dtime.to_le_bytes());
        b[0x18..0x1A].copy_from_slice(&self.gid.to_le_bytes());
        b[0x1A..0x1C].copy_from_slice(&self.links_count.to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&self.blocks.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&self.flags.to_le_bytes());
        b[0x28..0x28 + I_BLOCK_BYTES].copy_from_slice(&self.i_block);
        b[0x64..0x68].copy_from_slice(&self.generation.to_le_bytes());
        b[0x68..0x6C].copy_from_slice(&self.file_acl.to_le_bytes());
        b[0x6C..0x70].copy_from_slice(&self.size_high.to_le_bytes());
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_layout() -> Layout {
        Layout::for_fresh_image(32 * 1024 * 1024)
    }

    fn doctored(mut edit: impl FnMut(&mut [u8; 1024])) -> anyhow::Error {
        let mut bytes = Superblock::for_fresh_image(&small_layout(), [0; 16], "t", 0).to_bytes();
        edit(&mut bytes);
        Superblock::from_bytes(&bytes).expect_err("a doctored superblock must be refused")
    }

    #[test]
    fn a_superblock_naming_another_block_size_is_refused_and_says_which() {
        let err = doctored(|b| b[0x18..0x1C].copy_from_slice(&1u32.to_le_bytes()));
        assert!(
            format!("{err:#}").contains("2048-byte blocks"),
            "the refusal has to name the size it read: {err:#}"
        );
    }

    fn parsed(mut edit: impl FnMut(&mut Superblock)) -> Superblock {
        let mut sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "t", 0);
        edit(&mut sb);
        Superblock::from_bytes(&sb.to_bytes()).expect("a parsable superblock")
    }

    /// The guest mounts a volume read-write, so a killed run leaves it exactly like this — reading one must still work, or the volume becomes unusable rather than merely unresizable.
    #[test]
    fn a_superblock_a_killed_run_left_behind_still_parses() {
        let sb = parsed(|sb| {
            sb.state = 0;
            sb.feature_incompat |= 0x0000_0004;
        });
        assert_eq!(sb.state, 0);
        assert_eq!(sb.blocks_count, 8192, "its geometry is still readable");
    }

    #[test]
    fn a_volume_a_killed_run_left_dirty_is_not_safe_to_rewrite_and_says_what_to_do() {
        let err = parsed(|sb| sb.state = 0)
            .ensure_unmounted_cleanly()
            .expect_err("an unclean volume must not be rewritten");
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("not unmounted cleanly") && rendered.contains("run the sandbox once"),
            "the refusal has to tell the developer how to get their volume back: {rendered}"
        );
    }

    #[test]
    fn a_volume_with_features_this_writer_cannot_rewrite_is_refused_and_says_which() {
        let incompat = parsed(|sb| sb.feature_incompat |= 0x0000_0080)
            .ensure_rewritable_features()
            .expect_err("an unknown incompatible feature must not be rewritten");
        assert!(
            format!("{incompat:#}").contains("0x00000080"),
            "the refusal has to name the feature bits it cannot handle: {incompat:#}"
        );

        let ro_compat = parsed(|sb| sb.feature_ro_compat |= 0x0000_0400)
            .ensure_rewritable_features()
            .expect_err("an unknown read-only feature must not be rewritten");
        assert!(
            format!("{ro_compat:#}").contains("0x00000400"),
            "the refusal has to name the read-only bits it cannot handle: {ro_compat:#}"
        );
    }

    #[test]
    fn a_clean_volume_this_writer_produced_is_safe_to_rewrite() {
        let sb = parsed(|_| {});
        sb.ensure_rewritable_features()
            .expect("the writer's own fresh image must be growable");
        sb.ensure_unmounted_cleanly()
            .expect("the writer's own fresh image is settled");
    }

    /// A guest that was killed mid-write leaves this flag set, and its journal still holds transactions that would replay over anything a resize wrote.
    #[test]
    fn a_volume_whose_guest_journal_needs_replay_is_never_rewritable() {
        let err = parsed(|sb| sb.feature_incompat |= 0x0000_0004)
            .ensure_rewritable_features()
            .expect_err("an unreplayed journal must not be rewritten, marker or no marker");
        assert!(
            format!("{err:#}").contains("run the sandbox once"),
            "the developer's way out is the same as any other dirty volume, so say so: {err:#}"
        );
    }

    #[test]
    fn superblock_bytes_are_1024() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        assert_eq!(sb.to_bytes().len(), 1024);
    }

    #[test]
    fn superblock_magic_at_0x38() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        assert_eq!(u16::from_le_bytes([b[0x38], b[0x39]]), 0xEF53);
    }

    #[test]
    fn superblock_feature_flags_at_canonical_offsets() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let compat = u32::from_le_bytes([b[0x5C], b[0x5D], b[0x5E], b[0x5F]]);
        let incompat = u32::from_le_bytes([b[0x60], b[0x61], b[0x62], b[0x63]]);
        let ro_compat = u32::from_le_bytes([b[0x64], b[0x65], b[0x66], b[0x67]]);
        assert_eq!(compat, 0x0000_001C, "EXT_ATTR | HAS_JOURNAL | RESIZE_INODE");
        assert_eq!(incompat, 0x0000_0042, "FILETYPE | EXTENTS");
        assert_eq!(ro_compat, 0x0000_0003, "SPARSE_SUPER | LARGE_FILE");
    }

    #[test]
    fn superblock_first_data_block_is_zero_for_4k() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let fdb = u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]]);
        assert_eq!(fdb, 0);
    }

    #[test]
    fn superblock_desc_size_is_zero_when_64bit_clear() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let desc_size = u16::from_le_bytes([b[0xFE], b[0xFF]]);
        assert_eq!(desc_size, 0);
    }

    #[test]
    fn superblock_first_ino_is_11_in_rev1() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let first_ino = u32::from_le_bytes([b[0x54], b[0x55], b[0x56], b[0x57]]);
        assert_eq!(first_ino, 11);
    }

    #[test]
    fn superblock_inode_size_is_256() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let isize_field = u16::from_le_bytes([b[0x58], b[0x59]]);
        assert_eq!(isize_field, 256);
    }

    #[test]
    fn superblock_max_mnt_count_is_signed_minus_one() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        assert_eq!(b[0x36], 0xFF);
        assert_eq!(b[0x37], 0xFF);
    }

    #[test]
    fn superblock_uuid_at_0x68() {
        let uuid = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x32, 0x54, 0x76, 0x98, 0xBA,
            0xDC, 0xFE,
        ];
        let sb = Superblock::for_fresh_image(&small_layout(), uuid, "test", 0);
        let b = sb.to_bytes();
        assert_eq!(&b[0x68..0x78], &uuid);
    }

    #[test]
    fn superblock_volume_name_is_nul_padded() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "lns-upper", 0);
        let b = sb.to_bytes();
        assert_eq!(&b[0x78..0x81], b"lns-upper");
        for &x in &b[0x83..0x88] {
            assert_eq!(x, 0);
        }
    }

    #[test]
    fn superblock_long_volume_name_is_truncated() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "12345678901234567890", 0);
        let b = sb.to_bytes();
        assert_eq!(&b[0x78..0x88], b"1234567890123456");
    }

    #[test]
    fn superblock_for_backup_group_sets_block_group_nr() {
        let primary = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let backup = primary.for_backup_group(3);
        let pb = primary.to_bytes();
        let bb = backup.to_bytes();
        assert_eq!(u16::from_le_bytes([pb[0x5A], pb[0x5B]]), 0);
        assert_eq!(u16::from_le_bytes([bb[0x5A], bb[0x5B]]), 3);
        for i in 0..1024 {
            if (0x5A..0x5C).contains(&i) {
                continue;
            }
            assert_eq!(pb[i], bb[i], "byte 0x{i:X} should be equal");
        }
    }

    #[test]
    fn superblock_journal_area_is_zero_apart_from_the_inode_number() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        for (off, byte) in b[0xD0..0x108].iter().enumerate() {
            let abs = 0xD0 + off;
            if (0xE0..0xE4).contains(&abs) {
                continue;
            }
            assert_eq!(*byte, 0, "journal area byte 0x{abs:X} must be zero");
        }
    }

    #[test]
    fn the_superblock_advertises_an_internal_journal_on_inode_8() {
        let sb = Superblock::for_fresh_image(&small_layout(), [0; 16], "test", 0);
        let b = sb.to_bytes();
        let compat = u32::from_le_bytes([b[0x5C], b[0x5D], b[0x5E], b[0x5F]]);
        assert_eq!(
            compat & FEATURE_COMPAT_HAS_JOURNAL,
            FEATURE_COMPAT_HAS_JOURNAL
        );
        assert_eq!(
            u32::from_le_bytes([b[0xE0], b[0xE1], b[0xE2], b[0xE3]]),
            JOURNAL_INO,
            "s_journal_inum"
        );
        assert_eq!(
            &b[0xD0..0xE0],
            &[0u8; 16],
            "s_journal_uuid stays zero for an internal journal"
        );
        assert_eq!(
            &b[0xE4..0xE8],
            &[0u8; 4],
            "s_journal_dev stays zero for an internal journal"
        );
    }

    #[test]
    fn group_descriptor_bytes_are_32() {
        let gd = GroupDescriptor::default();
        assert_eq!(gd.to_bytes().len(), 32);
    }

    #[test]
    fn group_descriptor_field_offsets() {
        let gd = GroupDescriptor {
            block_bitmap: 0x11223344,
            inode_bitmap: 0x55667788,
            inode_table: 0x99AABBCC,
            free_blocks_count: 0xDDEE,
            free_inodes_count: 0x1234,
            used_dirs_count: 0x5678,
            flags: 0x9ABC,
            itable_unused: 0xDEAD,
        };
        let b = gd.to_bytes();
        assert_eq!(&b[0x00..0x04], &0x11223344u32.to_le_bytes());
        assert_eq!(&b[0x04..0x08], &0x55667788u32.to_le_bytes());
        assert_eq!(&b[0x08..0x0C], &0x99AABBCCu32.to_le_bytes());
        assert_eq!(&b[0x0C..0x0E], &0xDDEEu16.to_le_bytes());
        assert_eq!(&b[0x0E..0x10], &0x1234u16.to_le_bytes());
        assert_eq!(&b[0x10..0x12], &0x5678u16.to_le_bytes());
        assert_eq!(&b[0x12..0x14], &0x9ABCu16.to_le_bytes());
        assert_eq!(&b[0x1C..0x1E], &0xDEADu16.to_le_bytes());
    }

    #[test]
    fn inode_bytes_are_256() {
        let inode = Inode::default();
        assert_eq!(inode.to_bytes().len(), 256);
    }

    fn extent_header(i_block: &[u8; I_BLOCK_BYTES]) -> (u16, u16, u16, u16) {
        let magic = u16::from_le_bytes([i_block[0x00], i_block[0x01]]);
        let entries = u16::from_le_bytes([i_block[0x02], i_block[0x03]]);
        let max = u16::from_le_bytes([i_block[0x04], i_block[0x05]]);
        let depth = u16::from_le_bytes([i_block[0x06], i_block[0x07]]);
        (magic, entries, max, depth)
    }
    fn extent_record(i_block: &[u8; I_BLOCK_BYTES], idx: usize) -> (u32, u16, u64) {
        let off = 12 + idx * 12;
        let logical = u32::from_le_bytes([
            i_block[off],
            i_block[off + 1],
            i_block[off + 2],
            i_block[off + 3],
        ]);
        let length = u16::from_le_bytes([i_block[off + 4], i_block[off + 5]]);
        let hi = u16::from_le_bytes([i_block[off + 6], i_block[off + 7]]) as u64;
        let lo = u32::from_le_bytes([
            i_block[off + 8],
            i_block[off + 9],
            i_block[off + 10],
            i_block[off + 11],
        ]) as u64;
        (logical, length, (hi << 32) | lo)
    }

    #[test]
    fn root_directory_inode_has_correct_mode_and_links() {
        let root = Inode::root_directory(0, 516);
        assert_eq!(root.mode, S_IFDIR | 0o755, "mode = 0040755");
        assert_eq!(root.links_count, 3, "., .., lost+found");
        assert_eq!(root.size, BLOCK_SIZE, "one 4 KiB block");
        assert_eq!(root.blocks, 8, "i_blocks in 512-byte sectors");
        assert_eq!(root.flags, EXT4_EXTENTS_FL, "EXT4_EXTENTS_FL set");
        let (magic, entries, max, depth) = extent_header(&root.i_block);
        assert_eq!(magic, EXT4_EXTENT_MAGIC);
        assert_eq!(entries, 1, "one extent record");
        assert_eq!(max, INLINE_EXTENT_MAX);
        assert_eq!(depth, 0, "leaf node");
        let (logical, length, physical) = extent_record(&root.i_block, 0);
        assert_eq!(logical, 0);
        assert_eq!(length, 1);
        assert_eq!(physical, 516);
    }

    #[test]
    fn root_directory_serializes_mode_at_0x00() {
        let root = Inode::root_directory(0, 516);
        let b = root.to_bytes();
        let mode = u16::from_le_bytes([b[0x00], b[0x01]]);
        assert_eq!(mode, 0o0040755);
    }

    #[test]
    fn root_directory_serializes_extent_header_at_0x28() {
        let root = Inode::root_directory(0, 516);
        let b = root.to_bytes();
        let magic = u16::from_le_bytes([b[0x28], b[0x29]]);
        let entries = u16::from_le_bytes([b[0x2A], b[0x2B]]);
        assert_eq!(magic, EXT4_EXTENT_MAGIC);
        assert_eq!(entries, 1);
        let ee_block = u32::from_le_bytes([b[0x34], b[0x35], b[0x36], b[0x37]]);
        let ee_len = u16::from_le_bytes([b[0x38], b[0x39]]);
        let ee_start_lo = u32::from_le_bytes([b[0x3C], b[0x3D], b[0x3E], b[0x3F]]);
        assert_eq!(ee_block, 0);
        assert_eq!(ee_len, 1);
        assert_eq!(ee_start_lo, 516);
    }

    #[test]
    fn root_directory_serializes_extents_flag_at_0x20() {
        let root = Inode::root_directory(0, 516);
        let b = root.to_bytes();
        let flags = u32::from_le_bytes([b[0x20], b[0x21], b[0x22], b[0x23]]);
        assert_eq!(flags & EXT4_EXTENTS_FL, EXT4_EXTENTS_FL);
    }

    #[test]
    fn root_directory_i_blocks_is_512_byte_sectors_not_blocks() {
        let root = Inode::root_directory(0, 516);
        let b = root.to_bytes();
        let blocks_field = u32::from_le_bytes([b[0x1C], b[0x1D], b[0x1E], b[0x1F]]);
        assert_eq!(blocks_field, 8);
    }

    #[test]
    fn lost_found_inode_has_correct_mode_and_links() {
        let lf = Inode::lost_found_directory(0, 517);
        assert_eq!(lf.mode, S_IFDIR | 0o700, "mode = 0040700");
        assert_eq!(lf.links_count, 2, "., parent's lost+found entry");
        assert_eq!(lf.size, BLOCK_SIZE * 4, "4 reserved blocks");
        assert_eq!(lf.blocks, 8 * 4, "32 sectors");
        assert_eq!(lf.flags, EXT4_EXTENTS_FL, "EXT4_EXTENTS_FL set");
        let (magic, entries, _, depth) = extent_header(&lf.i_block);
        assert_eq!(magic, EXT4_EXTENT_MAGIC);
        assert_eq!(entries, 1, "one extent covers all 4 blocks");
        assert_eq!(depth, 0);
        let (logical, length, physical) = extent_record(&lf.i_block, 0);
        assert_eq!(logical, 0);
        assert_eq!(length, 4);
        assert_eq!(physical, 517);
    }

    #[test]
    fn the_journal_inode_is_a_regular_root_owned_file_the_workload_cannot_see() {
        let j = Inode::journal(0, 521, 4096);
        assert_eq!(j.mode, S_IFREG | 0o600, "mode = 0100600");
        assert_eq!(j.links_count, 1, "reachable only from the superblock");
        assert_eq!(j.uid, 0);
        assert_eq!(j.gid, 0);
        assert_eq!(j.flags, EXT4_EXTENTS_FL);
    }

    #[test]
    fn the_journal_inode_maps_the_reserved_range_as_one_extent() {
        let j = Inode::journal(0, 521, 4096);
        let (magic, entries, _, depth) = extent_header(&j.i_block);
        assert_eq!(magic, EXT4_EXTENT_MAGIC);
        assert_eq!(
            entries, 1,
            "the journal must stay contiguous: encode_inline_leaf holds at most 4 records"
        );
        assert_eq!(depth, 0);
        let (logical, length, physical) = extent_record(&j.i_block, 0);
        assert_eq!(logical, 0);
        assert_eq!(length, 4096);
        assert_eq!(physical, 521);
    }

    #[test]
    fn the_journal_inode_size_and_block_count_match_the_reservation() {
        let j = Inode::journal(0, 521, 4096);
        assert_eq!(j.size, 4096 * BLOCK_SIZE, "16 MiB still fits u32");
        assert_eq!(j.blocks, 4096 * (BLOCK_SIZE / 512));
    }

    #[test]
    fn inode_extension_area_is_zero() {
        let root = Inode::root_directory(0, 516);
        let b = root.to_bytes();
        for (off, byte) in b[0x80..0x100].iter().enumerate() {
            let abs = 0x80 + off;
            assert_eq!(*byte, 0, "extension byte 0x{abs:X} must be zero");
        }
    }
}
