pub const EXT2_SUPER_MAGIC: u16 = 0xEF53;

pub const BLOCK_SIZE: u32 = 4096;

pub const LOG_BLOCK_SIZE: u32 = 2;

// trailing 128 bytes in each inode MUST be zero because `RO_COMPAT_EXTRA_ISIZE` is not set — fsck treats non-zero as stale inode extension data.
pub const INODE_SIZE: u16 = 256;

pub const BLOCKS_PER_GROUP: u32 = 8 * BLOCK_SIZE;

pub const INODE_RATIO: u32 = 16384;

// `INCOMPAT_64BIT` is not set; the kernel and e2fsprogs both infer GROUP_DESC_SIZE = 32 from that — do not change this without setting INCOMPAT_64BIT.
pub const GROUP_DESC_SIZE: u32 = 32;

pub const DESCS_PER_BLOCK: u32 = BLOCK_SIZE / GROUP_DESC_SIZE;

pub const ADDRS_PER_BLOCK: u32 = BLOCK_SIZE / 4;

/// How much room a fresh image reserves to grow into, the factor mke2fs itself reserves for.
pub const GROWTH_FACTOR: u32 = 1024;

pub const FIRST_INO: u32 = 11;

pub const ROOT_INO: u32 = 2;
pub const LOST_FOUND_INO: u32 = 11;

pub const LOST_FOUND_BLOCKS: u32 = 4;

/// The resize inode's one double-indirect block, whose slots point at the reserved GDT blocks.
pub const RESIZE_DIND_BLOCKS: u32 = 1;

pub const RESIZE_INO: u32 = 7;

pub const EXT2_NDIR_BLOCKS: u32 = 12;
pub const EXT2_DIND_BLOCK: usize = 13;

pub const JOURNAL_INO: u32 = 8;

// JBD2 refuses to recover a journal shorter than 1024 blocks.
pub const JBD2_MIN_JOURNAL_BLOCKS: u32 = 1024;

pub const JOURNAL_TARGET_BLOCKS: u32 = 4096;

// Every JBD2 field is big-endian, unlike every other structure in this image.
pub const JBD2_MAGIC: u32 = 0xC03B_3998;
pub const JBD2_SUPERBLOCK_V2: u32 = 4;
pub const JBD2_FIRST_LOG_BLOCK: u32 = 1;
pub const JBD2_FIRST_SEQUENCE: u32 = 1;
pub const JBD2_SINGLE_USER: u32 = 1;

// `COMPAT_HAS_JOURNAL` MUST NOT be set without a valid JBD2 block at the reserved run — the kernel then refuses the rw mount outright.
pub const FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;

// `COMPAT_EXT_ATTR` MUST be set — without it fsck rejects `trusted.overlay.*` xattrs that overlayfs writes to the upper.
pub const FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;

// `INCOMPAT_FILETYPE` MUST be set — without it the kernel synthesises `DT_UNKNOWN` for every dirent and overlayfs whiteout detection breaks.
pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;

pub const FEATURE_INCOMPAT_EXTENTS: u32 = 0x0040;

// `RO_COMPAT_SPARSE_SUPER` MUST be set — the group_block_layout logic assumes SPARSE_SUPER placement; removing it silently corrupts the GDT.
pub const FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;

pub const FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;

// `COMPAT_RESIZE_INODE` MUST be set whenever `reserved_gdt_blocks` is non-zero — e2fsck refuses reserved blocks without it, and refuses a non-zero inode 7 without it.
pub const FEATURE_COMPAT_RESIZE_INODE: u32 = 0x0010;

pub const SB_FEATURE_COMPAT: u32 =
    FEATURE_COMPAT_EXT_ATTR | FEATURE_COMPAT_HAS_JOURNAL | FEATURE_COMPAT_RESIZE_INODE;
pub const SB_FEATURE_INCOMPAT: u32 = FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_EXTENTS;
pub const SB_FEATURE_RO_COMPAT: u32 = FEATURE_RO_COMPAT_SPARSE_SUPER | FEATURE_RO_COMPAT_LARGE_FILE;

pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

pub const EXT4_EXTENT_MAGIC: u16 = 0xF30A;

pub const INLINE_EXTENT_MAX: u16 = 4;

pub const EXT2_VALID_FS: u16 = 1;
pub const EXT2_ERRORS_CONTINUE: u16 = 1;
pub const EXT2_OS_LINUX: u32 = 0;
pub const EXT2_DYNAMIC_REV: u32 = 1;

pub const FT_DIR: u8 = 2;
pub const S_IFDIR: u16 = 0o0040000;
pub const S_IFREG: u16 = 0o0100000;
