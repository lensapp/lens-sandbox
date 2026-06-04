use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod real;
pub use real::spawn;

pub(crate) async fn tee<R, W>(reader: &mut R, log: &mut W, echo: bool) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 4096];
    loop {
        let n = reader.read(&mut buf).await.context("read hvc0")?;
        if n == 0 {
            break;
        }
        log.write_all(&buf[..n])
            .await
            .context("write hvc0 to console.log")?;
        if echo {
            let mut e = tokio::io::stderr();
            let _ = e.write_all(&buf[..n]).await;
        }
    }
    log.flush().await.context("flush console.log")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[tokio::test]
    async fn tee_copies_reader_into_log_until_eof() {
        let mut reader = io::Cursor::new(b"hello console".to_vec());
        let mut log: Vec<u8> = Vec::new();
        tee(&mut reader, &mut log, false).await.unwrap();
        assert_eq!(log, b"hello console");
    }

    #[tokio::test]
    async fn tee_with_echo_also_drives_the_echo_branch() {
        let mut reader = io::Cursor::new(b"x".to_vec());
        let mut log: Vec<u8> = Vec::new();
        tee(&mut reader, &mut log, true).await.unwrap();
        assert_eq!(log, b"x");
    }

    #[tokio::test]
    async fn tee_surfaces_read_errors() {
        let mut reader = tokio_test::io::Builder::new()
            .read_error(io::Error::other("boom"))
            .build();
        let mut log: Vec<u8> = Vec::new();
        let err = tee(&mut reader, &mut log, false).await.unwrap_err();
        assert!(format!("{err:#}").contains("read hvc0"), "got {err:#}");
    }

    #[tokio::test]
    async fn tee_surfaces_write_errors() {
        let mut reader = io::Cursor::new(b"x".to_vec());
        let mut log = tokio_test::io::Builder::new()
            .write_error(io::Error::other("boom"))
            .build();
        let err = tee(&mut reader, &mut log, false).await.unwrap_err();
        assert!(format!("{err:#}").contains("write hvc0"), "got {err:#}");
    }
}
