#[cfg(target_os = "linux")]
mod real;

#[cfg(target_os = "linux")]
pub use real::spawn_listener;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_header(body: &[u8]) -> Option<u16> {
    lns_session::decode_frame::<lns_session::ForwardHeader>(body)
        .ok()
        .map(|h| h.container_port)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptRecovery {
    Stop,
    Retry,
    Backoff,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn classify_accept_error(err: &std::io::Error) -> AcceptRecovery {
    if matches!(
        err.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
    ) {
        AcceptRecovery::Backoff
    } else if matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    ) {
        AcceptRecovery::Retry
    } else {
        AcceptRecovery::Stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_header_extracts_the_container_port() {
        let frame = lns_session::encode_frame(&lns_session::ForwardHeader {
            container_port: 3003,
        })
        .unwrap();
        assert_eq!(parse_header(&frame[4..]), Some(3003));
    }

    #[test]
    fn parse_header_rejects_an_empty_body() {
        assert_eq!(parse_header(&[]), None);
    }

    #[test]
    fn classify_accept_error_backs_off_on_resource_exhaustion() {
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
            let e = std::io::Error::from_raw_os_error(code);
            assert_eq!(
                classify_accept_error(&e),
                AcceptRecovery::Backoff,
                "code {code}"
            );
        }
    }

    #[test]
    fn classify_accept_error_retries_dropped_connections() {
        for kind in [
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::Interrupted,
        ] {
            assert_eq!(
                classify_accept_error(&std::io::Error::from(kind)),
                AcceptRecovery::Retry,
                "kind {kind:?}"
            );
        }
    }

    #[test]
    fn classify_accept_error_stops_on_unexpected_errors() {
        assert_eq!(
            classify_accept_error(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            AcceptRecovery::Stop
        );
        assert_eq!(
            classify_accept_error(&std::io::Error::from_raw_os_error(libc::EBADF)),
            AcceptRecovery::Stop
        );
    }
}
