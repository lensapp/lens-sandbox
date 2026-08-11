use crate::upperfs::constants::*;

pub fn superblock_block(journal_blocks: u32, uuid: [u8; 16]) -> Vec<u8> {
    let mut b = vec![0u8; BLOCK_SIZE as usize];
    b[0x00..0x04].copy_from_slice(&JBD2_MAGIC.to_be_bytes());
    b[0x04..0x08].copy_from_slice(&JBD2_SUPERBLOCK_V2.to_be_bytes());
    b[0x0C..0x10].copy_from_slice(&BLOCK_SIZE.to_be_bytes());
    b[0x10..0x14].copy_from_slice(&journal_blocks.to_be_bytes());
    b[0x14..0x18].copy_from_slice(&JBD2_FIRST_LOG_BLOCK.to_be_bytes());
    b[0x18..0x1C].copy_from_slice(&JBD2_FIRST_SEQUENCE.to_be_bytes());
    b[0x30..0x40].copy_from_slice(&uuid);
    b[0x40..0x44].copy_from_slice(&JBD2_SINGLE_USER.to_be_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_be32(b: &[u8], off: usize) -> u32 {
        u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    #[test]
    fn journal_superblock_header_is_big_endian_magic_and_v2_blocktype() {
        let b = superblock_block(1024, [0; 16]);
        assert_eq!(
            &b[0x00..0x04],
            &[0xC0, 0x3B, 0x39, 0x98],
            "JBD2 is the one big-endian structure in this image; a little-endian magic is unrecoverable"
        );
        assert_eq!(read_be32(&b, 0x04), JBD2_SUPERBLOCK_V2);
        assert_eq!(
            read_be32(&b, 0x08),
            0,
            "h_sequence is unused in a superblock"
        );
    }

    #[test]
    fn journal_superblock_describes_the_reserved_extent() {
        let b = superblock_block(4096, [0; 16]);
        assert_eq!(read_be32(&b, 0x0C), BLOCK_SIZE, "s_blocksize");
        assert_eq!(read_be32(&b, 0x10), 4096, "s_maxlen");
        assert_eq!(
            read_be32(&b, 0x14),
            1,
            "s_first: block 0 is this superblock"
        );
        assert_eq!(
            read_be32(&b, 0x18),
            1,
            "s_sequence: first commit id expected"
        );
        assert_eq!(
            read_be32(&b, 0x1C),
            0,
            "s_start: an empty journal has no log"
        );
        assert_eq!(read_be32(&b, 0x40), 1, "s_nr_users: one filesystem");
    }

    #[test]
    fn journal_superblock_carries_the_filesystem_uuid_for_its_single_user() {
        let uuid = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x32, 0x54, 0x76, 0x98, 0xBA,
            0xDC, 0xFE,
        ];
        let b = superblock_block(1024, uuid);
        assert_eq!(&b[0x30..0x40], &uuid);
    }

    #[test]
    fn journal_superblock_declares_no_features_so_any_kernel_can_recover_it() {
        let b = superblock_block(1024, [0xAA; 16]);
        assert_eq!(read_be32(&b, 0x24), 0, "s_feature_compat");
        assert_eq!(read_be32(&b, 0x28), 0, "s_feature_incompat");
        assert_eq!(read_be32(&b, 0x2C), 0, "s_feature_ro_compat");
        assert_eq!(b[0x50], 0, "s_checksum_type");
        assert_eq!(read_be32(&b, 0xFC), 0, "s_checksum");
    }

    #[test]
    fn journal_superblock_is_exactly_one_block_and_otherwise_zero() {
        let b = superblock_block(4096, [0xAA; 16]);
        assert_eq!(b.len(), BLOCK_SIZE as usize);
        let set: Vec<usize> = (0..b.len())
            .filter(|&i| b[i] != 0)
            .filter(|i| !(0x00..0x08).contains(i))
            .filter(|i| !(0x0C..0x20).contains(i))
            .filter(|i| !(0x30..0x44).contains(i))
            .collect();
        assert!(set.is_empty(), "unexpected non-zero bytes at {set:?}");
    }
}
