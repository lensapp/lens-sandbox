use crate::upperfs::constants::{EXT4_EXTENT_MAGIC, INLINE_EXTENT_MAX};

pub const I_BLOCK_BYTES: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtentLeaf {
    pub logical_block: u32,
    pub length: u16,
    pub physical_block: u64,
}

pub fn encode_inline_leaf(extents: &[ExtentLeaf]) -> [u8; I_BLOCK_BYTES] {
    assert!(
        extents.len() <= INLINE_EXTENT_MAX as usize,
        "inline extent layout caps at {INLINE_EXTENT_MAX} records, got {}",
        extents.len()
    );

    let mut out = [0u8; I_BLOCK_BYTES];

    out[0x00..0x02].copy_from_slice(&EXT4_EXTENT_MAGIC.to_le_bytes());
    out[0x02..0x04].copy_from_slice(&(extents.len() as u16).to_le_bytes());
    out[0x04..0x06].copy_from_slice(&INLINE_EXTENT_MAX.to_le_bytes());
    out[0x06..0x08].copy_from_slice(&0u16.to_le_bytes());
    out[0x08..0x0C].copy_from_slice(&0u32.to_le_bytes());

    for (i, e) in extents.iter().enumerate() {
        assert!(
            e.length >= 1 && e.length <= 32768,
            "extent length {} out of range (1..=32768)",
            e.length
        );
        assert!(
            e.physical_block < 1u64 << 48,
            "physical block {} exceeds 48-bit extent address space",
            e.physical_block
        );
        let off = 12 + i * 12;
        let ee_start_lo = (e.physical_block & 0xFFFF_FFFF) as u32;
        let ee_start_hi = ((e.physical_block >> 32) & 0xFFFF) as u16;
        out[off..off + 4].copy_from_slice(&e.logical_block.to_le_bytes());
        out[off + 4..off + 6].copy_from_slice(&e.length.to_le_bytes());
        out[off + 6..off + 8].copy_from_slice(&ee_start_hi.to_le_bytes());
        out[off + 8..off + 12].copy_from_slice(&ee_start_lo.to_le_bytes());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u16(buf: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([buf[off], buf[off + 1]])
    }
    fn read_u32(buf: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    #[test]
    fn encode_single_extent_header_layout() {
        let bytes = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 1,
            physical_block: 516,
        }]);
        assert_eq!(bytes.len(), 60);
        assert_eq!(read_u16(&bytes, 0x00), EXT4_EXTENT_MAGIC);
        assert_eq!(read_u16(&bytes, 0x02), 1, "eh_entries");
        assert_eq!(read_u16(&bytes, 0x04), INLINE_EXTENT_MAX, "eh_max = 4");
        assert_eq!(read_u16(&bytes, 0x06), 0, "eh_depth = 0 (leaf)");
        assert_eq!(read_u32(&bytes, 0x08), 0, "eh_generation = 0");
    }

    #[test]
    fn encode_single_extent_record_layout() {
        let bytes = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 4,
            physical_block: 517,
        }]);
        assert_eq!(read_u32(&bytes, 12), 0, "ee_block");
        assert_eq!(read_u16(&bytes, 16), 4, "ee_len");
        assert_eq!(read_u16(&bytes, 18), 0, "ee_start_hi (low physical)");
        assert_eq!(read_u32(&bytes, 20), 517, "ee_start_lo");
    }

    #[test]
    fn encode_multiple_extents_pack_contiguously() {
        let bytes = encode_inline_leaf(&[
            ExtentLeaf {
                logical_block: 0,
                length: 2,
                physical_block: 1000,
            },
            ExtentLeaf {
                logical_block: 2,
                length: 3,
                physical_block: 2000,
            },
            ExtentLeaf {
                logical_block: 5,
                length: 1,
                physical_block: 3000,
            },
        ]);
        assert_eq!(read_u16(&bytes, 0x02), 3, "eh_entries = 3");
        assert_eq!(read_u32(&bytes, 12), 0);
        assert_eq!(read_u16(&bytes, 16), 2);
        assert_eq!(read_u32(&bytes, 20), 1000);
        assert_eq!(read_u32(&bytes, 24), 2);
        assert_eq!(read_u16(&bytes, 28), 3);
        assert_eq!(read_u32(&bytes, 32), 2000);
        assert_eq!(read_u32(&bytes, 36), 5);
        assert_eq!(read_u16(&bytes, 40), 1);
        assert_eq!(read_u32(&bytes, 44), 3000);
        for (i, &b) in bytes[48..60].iter().enumerate() {
            assert_eq!(b, 0, "unused record byte at offset {} must be zero", 48 + i);
        }
    }

    #[test]
    fn encode_full_inline_capacity_4_records() {
        let bytes = encode_inline_leaf(&[
            ExtentLeaf {
                logical_block: 0,
                length: 1,
                physical_block: 100,
            },
            ExtentLeaf {
                logical_block: 1,
                length: 1,
                physical_block: 200,
            },
            ExtentLeaf {
                logical_block: 2,
                length: 1,
                physical_block: 300,
            },
            ExtentLeaf {
                logical_block: 3,
                length: 1,
                physical_block: 400,
            },
        ]);
        assert_eq!(read_u16(&bytes, 0x02), 4);
        assert_eq!(read_u32(&bytes, 56), 400);
    }

    #[test]
    fn encode_empty_list_yields_header_only() {
        let bytes = encode_inline_leaf(&[]);
        assert_eq!(read_u16(&bytes, 0x00), EXT4_EXTENT_MAGIC);
        assert_eq!(read_u16(&bytes, 0x02), 0, "eh_entries = 0");
        assert_eq!(read_u16(&bytes, 0x04), INLINE_EXTENT_MAX);
        for (i, &b) in bytes[12..].iter().enumerate() {
            assert_eq!(b, 0, "record byte at offset {} must be zero", 12 + i);
        }
    }

    #[test]
    fn physical_block_high_low_split() {
        let phys = 0x0000_1234_5678_9ABCu64;
        let bytes = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 1,
            physical_block: phys,
        }]);
        let lo = read_u32(&bytes, 20) as u64;
        let hi = read_u16(&bytes, 18) as u64;
        let reconstructed = (hi << 32) | lo;
        assert_eq!(reconstructed, phys);
    }

    #[test]
    #[should_panic(expected = "inline extent layout caps at")]
    fn encode_more_than_4_records_panics() {
        let big = vec![
            ExtentLeaf {
                logical_block: 0,
                length: 1,
                physical_block: 100,
            };
            5
        ];
        let _ = encode_inline_leaf(&big);
    }

    #[test]
    #[should_panic(expected = "extent length")]
    fn encode_zero_length_panics() {
        let _ = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 0,
            physical_block: 100,
        }]);
    }

    #[test]
    #[should_panic(expected = "exceeds 48-bit")]
    fn encode_physical_block_past_48_bits_panics() {
        let _ = encode_inline_leaf(&[ExtentLeaf {
            logical_block: 0,
            length: 1,
            physical_block: 1u64 << 48,
        }]);
    }
}
