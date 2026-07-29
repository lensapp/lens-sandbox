use std::io::{self, Read};

const GIB: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 16 * GIB;
const MAX_ARCHIVE_ENTRIES: usize = 4_000_000;

#[derive(Clone, Copy)]
pub(crate) struct ArchiveLimits {
    pub(crate) bytes: u64,
    pub(crate) entries: usize,
}

impl ArchiveLimits {
    pub(crate) const PRODUCTION: Self = Self {
        bytes: MAX_ARCHIVE_BYTES,
        entries: MAX_ARCHIVE_ENTRIES,
    };
}

pub(crate) struct LimitedReader<R> {
    inner: R,
    pub(crate) read: u64,
    limit: u64,
    subject: &'static str,
}

impl<R: Read> LimitedReader<R> {
    pub(crate) fn new(inner: R, limit: u64, subject: &'static str) -> Self {
        Self {
            inner,
            read: 0,
            limit,
            subject,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.limit - self.read;
        if remaining == 0 {
            return match self.inner.read(&mut [0u8; 1])? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "{} exceeds the {}-byte cap",
                    self.subject, self.limit
                ))),
            };
        }
        let cap = remaining.min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..cap])?;
        self.read += n as u64;
        Ok(n)
    }
}
