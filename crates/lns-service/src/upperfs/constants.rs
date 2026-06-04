pub const EXT2_SUPER_MAGIC: u16 = 0xEF53;

pub const BLOCK_SIZE: u32 = 4096;

pub const LOG_BLOCK_SIZE: u32 = 2;

// trailing 128 bytes in each inode MUST be zero because `RO_COMPAT_EXTRA_ISIZE` is not set — fsck treats non-zero as stale inode extension data.
pub const INODE_SIZE: u16 = 256;

pub const BLOCKS_PER_GROUP: u32 = 8 * BLOCK_SIZE;

pub const INODE_RATIO: u32 = 16384;

// `INCOMPAT_64BIT` is not set; the kernel and e2fsprogs both infer GROUP_DESC_SIZE = 32 from that — do not change this without setting INCOMPAT_64BIT.
pub const GROUP_DESC_SIZE: u32 = 32;

pub const FIRST_INO: u32 = 11;

pub const ROOT_INO: u32 = 2;
pub const LOST_FOUND_INO: u32 = 11;

pub const LOST_FOUND_BLOCKS: u32 = 4;

// `COMPAT_EXT_ATTR` MUST be set — without it fsck rejects `trusted.overlay.*` xattrs that overlayfs writes to the upper.
pub const FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;

// `INCOMPAT_FILETYPE` MUST be set — without it the kernel synthesises `DT_UNKNOWN` for every dirent and overlayfs whiteout detection breaks.
pub const FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;

pub const FEATURE_INCOMPAT_EXTENTS: u32 = 0x0040;

// `RO_COMPAT_SPARSE_SUPER` MUST be set — the group_block_layout logic assumes SPARSE_SUPER placement; removing it silently corrupts the GDT.
pub const FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;

pub const FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;

pub const SB_FEATURE_COMPAT: u32 = FEATURE_COMPAT_EXT_ATTR;
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
