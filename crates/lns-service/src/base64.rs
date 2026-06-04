pub(crate) fn encode(input: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let chunks = input.chunks_exact(3);
    let remainder = chunks.remainder();
    for chunk in chunks {
        let b0 = chunk[0] as u32;
        let b1 = chunk[1] as u32;
        let b2 = chunk[2] as u32;
        let v = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((v >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((v >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((v >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(v & 0x3f) as usize] as char);
    }
    match remainder.len() {
        1 => {
            let b0 = remainder[0] as u32;
            let v = b0 << 16;
            out.push(ALPHA[((v >> 18) & 0x3f) as usize] as char);
            out.push(ALPHA[((v >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b0 = remainder[0] as u32;
            let b1 = remainder[1] as u32;
            let v = (b0 << 16) | (b1 << 8);
            out.push(ALPHA[((v >> 18) & 0x3f) as usize] as char);
            out.push(ALPHA[((v >> 12) & 0x3f) as usize] as char);
            out.push(ALPHA[((v >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_known_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encode_handles_high_bytes() {
        assert_eq!(encode(&[0xff, 0xee, 0xdd]), "/+7d");
        assert_eq!(encode(&[0x00, 0x10, 0x83]), "ABCD");
    }
}
