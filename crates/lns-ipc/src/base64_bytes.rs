//! Bytes travel as base64 because the IPC frame is JSON, where a byte array would otherwise become a list of 350k numbers.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    encode(bytes).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(deserializer)?;
    decode(&text).ok_or_else(|| serde::de::Error::custom("invalid base64"))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Strict: the length must be a multiple of four and padding may only close the final quantum, so one payload has exactly one encoding.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let body = text.trim_end_matches('=');
    if text.len() - body.len() > 2 {
        return None;
    }
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for c in body.bytes() {
        let value = ALPHABET.iter().position(|a| *a == c)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_length_round_trips_so_padding_is_handled() {
        for len in 0..=32usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
            assert_eq!(decode(&encode(&bytes)).unwrap(), bytes, "len {len}");
        }
    }

    #[test]
    fn a_high_byte_keyring_style_payload_round_trips() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn padding_lands_only_where_the_chunk_is_short() {
        assert_eq!(encode(b"abc"), "YWJj");
        assert_eq!(encode(b"ab"), "YWI=");
        assert_eq!(encode(b"a"), "YQ==");
    }

    #[test]
    fn a_character_outside_the_alphabet_is_refused_rather_than_skipped() {
        assert!(decode("YW!j").is_none());
    }

    #[test]
    fn padding_in_the_middle_is_refused_rather_than_silently_truncating() {
        assert!(decode("YQ==YQ==").is_none());
    }

    #[test]
    fn more_padding_than_a_quantum_can_carry_is_refused() {
        // A quantum encodes at most two padding characters; more cannot describe any byte length.
        for bad in ["A===", "===="] {
            assert!(decode(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_length_that_is_not_a_whole_quantum_is_refused() {
        for bad in ["YQ", "YQ=", "YWJjY"] {
            assert!(decode(bad).is_none(), "{bad:?} must be refused");
        }
    }
}
