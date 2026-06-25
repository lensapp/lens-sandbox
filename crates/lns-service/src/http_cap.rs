use anyhow::{Result, bail};

#[derive(Debug)]
pub(crate) struct CappedBuffer {
    buf: Vec<u8>,
    max: u64,
}

impl CappedBuffer {
    pub(crate) fn new(content_length: Option<u64>, max: u64, url: &str) -> Result<Self> {
        if let Some(len) = content_length
            && len > max
        {
            bail!("{url} declared {len} bytes, over the {max}-byte download cap");
        }
        Ok(Self {
            buf: Vec::new(),
            max,
        })
    }

    pub(crate) fn push<E: std::fmt::Display>(
        &mut self,
        chunk: std::result::Result<Vec<u8>, E>,
        url: &str,
    ) -> Result<()> {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => bail!("reading body from {url}: {e}"),
        };
        if self.buf.len() as u64 + chunk.len() as u64 > self.max {
            bail!("{url} body exceeded the {}-byte download cap", self.max);
        }
        self.buf.extend_from_slice(&chunk);
        Ok(())
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_length_over_the_cap_is_refused_before_any_body_is_read() {
        let err = CappedBuffer::new(Some(101), 100, "https://h/x").unwrap_err();
        assert!(format!("{err:#}").contains("over the 100-byte download cap"));
    }

    #[test]
    fn an_absent_or_within_cap_declared_length_is_accepted() {
        assert!(CappedBuffer::new(None, 100, "https://h/x").is_ok());
        assert!(CappedBuffer::new(Some(100), 100, "https://h/x").is_ok());
    }

    #[test]
    fn chunks_within_the_cap_accumulate_in_order() {
        let mut b = CappedBuffer::new(None, 10, "https://h/x").unwrap();
        b.push(Ok::<_, String>(vec![1, 2, 3]), "https://h/x")
            .unwrap();
        b.push(Ok::<_, String>(vec![4, 5]), "https://h/x").unwrap();
        assert_eq!(b.into_inner(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_chunk_that_would_cross_the_cap_is_refused_even_with_no_declared_length() {
        let mut b = CappedBuffer::new(None, 4, "https://h/x").unwrap();
        b.push(Ok::<_, String>(vec![1, 2, 3]), "https://h/x")
            .unwrap();
        let err = b
            .push(Ok::<_, String>(vec![4, 5]), "https://h/x")
            .unwrap_err();
        assert!(format!("{err:#}").contains("exceeded the 4-byte download cap"));
    }

    #[test]
    fn a_transport_error_chunk_is_surfaced_with_the_url() {
        let mut b = CappedBuffer::new(None, 100, "https://h/x").unwrap();
        let err = b
            .push(Err::<Vec<u8>, _>("connection reset"), "https://h/x")
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("reading body from https://h/x"), "{msg}");
        assert!(msg.contains("connection reset"), "{msg}");
    }
}
