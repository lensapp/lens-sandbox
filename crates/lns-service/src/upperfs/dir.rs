#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name: Vec<u8>,
}

impl DirEntry {
    pub fn new(inode: u32, file_type: u8, name: &str) -> Self {
        Self {
            inode,
            file_type,
            name: name.as_bytes().to_vec(),
        }
    }

    pub fn min_rec_len(&self) -> u16 {
        let raw = 8 + self.name.len();
        ((raw + 3) & !3) as u16
    }
}

#[derive(Debug, Default)]
pub struct DirBlock {
    entries: Vec<DirEntry>,
}

impl DirBlock {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, entry: DirEntry) {
        self.entries.push(entry);
    }

    pub fn sentinel(block_size: u32) -> Vec<u8> {
        let mut out = vec![0u8; block_size as usize];
        let rec_len = u16::try_from(block_size).expect("block size fits in u16");
        out[4..6].copy_from_slice(&rec_len.to_le_bytes());
        out
    }

    pub fn to_bytes(&self, block_size: u32) -> Vec<u8> {
        assert!(
            !self.entries.is_empty(),
            "DirBlock::to_bytes requires at least one entry; use sentinel() for empty blocks"
        );
        let mut out = vec![0u8; block_size as usize];
        let mut offset = 0usize;
        let total = block_size as usize;
        for (i, e) in self.entries.iter().enumerate() {
            let is_last = i + 1 == self.entries.len();
            let min = e.min_rec_len() as usize;
            let rec_len = if is_last { total - offset } else { min };
            assert!(
                rec_len >= min,
                "entry {i} ({:?}) needs at least {min} bytes, only {rec_len} available",
                String::from_utf8_lossy(&e.name)
            );
            assert!(
                rec_len % 4 == 0,
                "rec_len for entry {i} = {rec_len} is not 4-aligned"
            );
            assert!(
                offset + rec_len <= total,
                "entry {i} overflows block: offset {offset} + rec_len {rec_len} > {total}"
            );
            let rec_len_u16 = u16::try_from(rec_len).expect("rec_len fits in u16");
            out[offset..offset + 4].copy_from_slice(&e.inode.to_le_bytes());
            out[offset + 4..offset + 6].copy_from_slice(&rec_len_u16.to_le_bytes());
            out[offset + 6] = u8::try_from(e.name.len()).expect("name_len fits in u8");
            out[offset + 7] = e.file_type;
            out[offset + 8..offset + 8 + e.name.len()].copy_from_slice(&e.name);
            offset += rec_len;
        }
        debug_assert_eq!(offset, total, "did not fill block");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upperfs::constants::{BLOCK_SIZE, FT_DIR, LOST_FOUND_INO, ROOT_INO};

    #[test]
    fn min_rec_len_pads_to_4_byte_boundary() {
        assert_eq!(DirEntry::new(2, FT_DIR, ".").min_rec_len(), 12);
        assert_eq!(DirEntry::new(2, FT_DIR, "..").min_rec_len(), 12);
        assert_eq!(DirEntry::new(11, FT_DIR, "lost+found").min_rec_len(), 20);
        assert_eq!(DirEntry::new(1, 1, "abc").min_rec_len(), 12);
        assert_eq!(DirEntry::new(1, 1, "abcd").min_rec_len(), 12);
        assert_eq!(DirEntry::new(1, 1, "abcde").min_rec_len(), 16);
    }

    #[test]
    fn root_directory_block_byte_layout() {
        let mut blk = DirBlock::new();
        blk.push(DirEntry::new(ROOT_INO, FT_DIR, "."));
        blk.push(DirEntry::new(ROOT_INO, FT_DIR, ".."));
        blk.push(DirEntry::new(LOST_FOUND_INO, FT_DIR, "lost+found"));
        let b = blk.to_bytes(BLOCK_SIZE);
        assert_eq!(b.len(), BLOCK_SIZE as usize);

        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 2);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 12);
        assert_eq!(b[6], 1);
        assert_eq!(b[7], FT_DIR);
        assert_eq!(&b[8..9], b".");
        assert_eq!(&b[9..12], &[0, 0, 0]);

        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), 2);
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 12);
        assert_eq!(b[18], 2);
        assert_eq!(b[19], FT_DIR);
        assert_eq!(&b[20..22], b"..");
        assert_eq!(&b[22..24], &[0, 0]);

        assert_eq!(u32::from_le_bytes([b[24], b[25], b[26], b[27]]), 11);
        assert_eq!(u16::from_le_bytes([b[28], b[29]]), 4072);
        assert_eq!(b[30], 10);
        assert_eq!(b[31], FT_DIR);
        assert_eq!(&b[32..42], b"lost+found");
        for (i, &x) in b[42..].iter().enumerate() {
            assert_eq!(x, 0, "padding byte at offset {} must be zero", 42 + i);
        }
    }

    #[test]
    fn lost_found_first_block_byte_layout() {
        let mut blk = DirBlock::new();
        blk.push(DirEntry::new(LOST_FOUND_INO, FT_DIR, "."));
        blk.push(DirEntry::new(ROOT_INO, FT_DIR, ".."));
        let b = blk.to_bytes(BLOCK_SIZE);

        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 11);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), 12);
        assert_eq!(b[6], 1);
        assert_eq!(b[7], FT_DIR);
        assert_eq!(&b[8..9], b".");

        assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), 2);
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 4084);
        assert_eq!(b[18], 2);
        assert_eq!(b[19], FT_DIR);
        assert_eq!(&b[20..22], b"..");
    }

    #[test]
    fn sum_of_rec_lens_equals_block_size() {
        let mut blk = DirBlock::new();
        blk.push(DirEntry::new(2, FT_DIR, "."));
        blk.push(DirEntry::new(2, FT_DIR, ".."));
        blk.push(DirEntry::new(11, FT_DIR, "lost+found"));
        let b = blk.to_bytes(BLOCK_SIZE);
        let mut offset = 0usize;
        let mut count = 0;
        while offset < BLOCK_SIZE as usize {
            let rec_len = u16::from_le_bytes([b[offset + 4], b[offset + 5]]) as usize;
            assert!(rec_len > 0, "rec_len 0 mid-block at offset {offset}");
            offset += rec_len;
            count += 1;
        }
        assert_eq!(
            offset, BLOCK_SIZE as usize,
            "did not land exactly at block end"
        );
        assert_eq!(count, 3);
    }

    #[test]
    fn sentinel_block_has_inode_zero_and_full_rec_len() {
        let b = DirBlock::sentinel(BLOCK_SIZE);
        assert_eq!(b.len(), BLOCK_SIZE as usize);
        assert_eq!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 0);
        assert_eq!(u16::from_le_bytes([b[4], b[5]]), BLOCK_SIZE as u16);
        assert_eq!(b[6], 0);
        assert_eq!(b[7], 0);
        for &x in &b[8..] {
            assert_eq!(x, 0);
        }
    }

    #[test]
    #[should_panic(expected = "at least one entry")]
    fn empty_dirblock_panics() {
        let _ = DirBlock::new().to_bytes(BLOCK_SIZE);
    }

    #[test]
    #[should_panic(expected = "needs at least")]
    fn last_entry_too_small_panics_with_lossy_decoded_name() {
        let mut blk = DirBlock::new();
        blk.push(DirEntry::new(2, FT_DIR, "a"));
        blk.push(DirEntry {
            inode: 3,
            file_type: FT_DIR,
            name: vec![0xff, 0xfe],
        });
        let _ = blk.to_bytes(16);
    }
}
