use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpStream};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(15);

pub fn request(endpoint: SocketAddrV4, method: &str, path: &str) -> Result<(u16, String)> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V4(endpoint), CONNECT_TIMEOUT)
        .with_context(|| format!("connect to {endpoint}"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {endpoint}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    parse_response(&String::from_utf8_lossy(&answer))
}

fn parse_response(text: &str) -> Result<(u16, String)> {
    let (head, body) = text
        .split_once("\r\n\r\n")
        .context("the answer carries no HTTP head")?;
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .context("the answer carries no HTTP status")?;
    Ok((status, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_carries_its_status_and_its_body() {
        assert_eq!(
            parse_response("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap(),
            (200, "{}".to_string())
        );
        assert!(parse_response("HTTP/1.1 200 OK").is_err());
        assert!(parse_response("nonsense\r\n\r\nbody").is_err());
    }
}
