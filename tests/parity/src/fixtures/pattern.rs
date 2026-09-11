use sha2::{Digest, Sha256};

pub const MIB: u64 = 1024 * 1024;

pub const ZEROS_100MIB_SHA256: &str =
    "20492a4d0d84f8beb1767f6616229f85d44c2827b64bdbfb260ee12fa1109e0e";

pub const ZEROS_10MIB_SHA256: &str =
    "e5b844cc57f57094ea4585e235f36c78c1cd222262bb89d53c94dcb4d6b3e55d";

pub fn pattern_byte(offset: u64) -> u8 {
    ((offset.wrapping_mul(7).wrapping_add(3)) % 256) as u8
}

pub fn pattern_chunk(offset: u64, len: usize) -> Vec<u8> {
    (0..len as u64).map(|i| pattern_byte(offset + i)).collect()
}

pub fn pattern_sha256(len: u64) -> String {
    let mut hasher = Sha256::new();
    let mut offset = 0u64;
    while offset < len {
        let take = std::cmp::min(64 * 1024, len - offset) as usize;
        hasher.update(pattern_chunk(offset, take));
        offset += take as u64;
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
pub fn zeros_sha256(len: u64) -> String {
    let mut hasher = Sha256::new();
    let chunk = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    while offset < len {
        let take = std::cmp::min(chunk.len() as u64, len - offset) as usize;
        hasher.update(&chunk[..take]);
        offset += take as u64;
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pattern_is_the_one_the_guest_reproduces() {
        assert_eq!(pattern_byte(0), 3);
        assert_eq!(pattern_byte(1), 10);
        assert_eq!(pattern_byte(36), 255);
        assert_eq!(pattern_chunk(2, 3), vec![17, 24, 31]);
    }

    #[test]
    fn a_chunked_pattern_hash_equals_the_whole() {
        let whole = {
            let mut hasher = Sha256::new();
            hasher.update(pattern_chunk(0, 300_000));
            hex::encode(hasher.finalize())
        };
        assert_eq!(pattern_sha256(300_000), whole);
    }

    #[test]
    fn the_hundred_mebibyte_zero_constant_is_the_hash_of_a_hundred_mebibytes_of_zeros() {
        assert_eq!(zeros_sha256(100 * MIB), ZEROS_100MIB_SHA256);
        assert_eq!(zeros_sha256(10 * MIB), ZEROS_10MIB_SHA256);
    }
}
