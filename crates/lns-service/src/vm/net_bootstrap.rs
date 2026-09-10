#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::net::Ipv4Addr;
use std::time::Duration;

use lns_session::{BrokerExitReason, ClientFrame, GuestNet, ServerFrame};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// What the guest said about the plan the host sent it. A refusal is an answer: the guest is reachable and has named why it cannot take any of the offered addresses.
#[derive(Debug, PartialEq, Eq)]
pub enum Bootstrapped {
    Applied(Ipv4Addr),
    Refused(BrokerExitReason),
}

/// Why the host never learned what the guest did with the plan. None of these start a workload.
#[derive(Debug)]
pub enum BootstrapError {
    Timeout(Duration),
    Disconnected,
    Unoffered(Ipv4Addr),
    Malformed(String),
    Unexpected(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(after) => write!(
                f,
                "the guest did not report an address within {after:?} of being sent one"
            ),
            Self::Disconnected => write!(
                f,
                "the guest closed the bootstrap channel without reporting an address"
            ),
            Self::Unoffered(address) => {
                write!(f, "the guest reported {address}, which no run reserved")
            }
            Self::Malformed(value) => {
                write!(f, "the guest reported {value:?}, which is not an address")
            }
            Self::Unexpected(what) => write!(
                f,
                "the guest sent {what} before it reported the address it took"
            ),
            Self::Io(e) => write!(f, "the guest could not be sent its address plan: {e}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

/// The step between a started VMM and a started workload: the guest is told which addresses the host holds for it, and answers with the one it took or with why it took none.
pub async fn configure<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    net: &GuestNet,
    reply_within: Duration,
) -> Result<Bootstrapped, BootstrapError> {
    let mut stream = stream;
    let bytes = lns_session::encode_frame(&ClientFrame::ConfigureNetwork(net.clone()))
        .map_err(|e| BootstrapError::Io(std::io::Error::other(e.to_string())))?;
    stream.write_all(&bytes).await.map_err(BootstrapError::Io)?;
    stream.flush().await.map_err(BootstrapError::Io)?;
    let frame = tokio::time::timeout(
        reply_within,
        super::session_client::read_one_frame(&mut stream),
    )
    .await
    .map_err(|_| BootstrapError::Timeout(reply_within))?
    .map_err(|e| BootstrapError::Malformed(format!("{e:#}")))?;
    match frame {
        None => Err(BootstrapError::Disconnected),
        Some(ServerFrame::NetworkApplied { address }) => {
            let taken: Ipv4Addr = address
                .parse()
                .map_err(|_| BootstrapError::Malformed(address.clone()))?;
            if !net.candidates.contains(&taken) {
                return Err(BootstrapError::Unoffered(taken));
            }
            Ok(Bootstrapped::Applied(taken))
        }
        Some(ServerFrame::Refused(reason)) => Ok(Bootstrapped::Refused(reason)),
        Some(ServerFrame::StdoutBytes(_)) => Err(BootstrapError::Unexpected("workload output")),
        Some(ServerFrame::StderrBytes(_)) => Err(BootstrapError::Unexpected("workload output")),
        Some(ServerFrame::ExitStatus(_)) => Err(BootstrapError::Unexpected("a workload exit")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::{BrokerExitReason, ClientFrame, GuestNet, ServerFrame};
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    fn plan() -> GuestNet {
        GuestNet {
            candidates: vec![
                Ipv4Addr::new(192, 168, 64, 254),
                Ipv4Addr::new(192, 168, 64, 253),
            ],
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec![Ipv4Addr::new(192, 168, 64, 1)],
        }
    }

    /// A guest that answers with `reply` once it has read the plan the host sent it.
    async fn guest(
        mut server: tokio::io::DuplexStream,
        reply: Option<ServerFrame>,
    ) -> Option<ClientFrame> {
        let mut len = [0u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut server, &mut len)
            .await
            .ok()?;
        let mut body = vec![0u8; lns_session::decode_length_prefix(&len).ok()?];
        tokio::io::AsyncReadExt::read_exact(&mut server, &mut body)
            .await
            .ok()?;
        let sent: ClientFrame = lns_session::decode_frame(&body).ok()?;
        if let Some(frame) = reply {
            let bytes = lns_session::encode_frame(&frame).ok()?;
            server.write_all(&bytes).await.ok()?;
            server.flush().await.ok()?;
            // The guest keeps the channel open, as a booting one does.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Some(sent)
    }

    #[tokio::test]
    async fn the_guest_is_sent_the_plan_and_reports_the_address_it_took() {
        let (host, server) = tokio::io::duplex(4096);
        let guest = tokio::spawn(guest(
            server,
            Some(ServerFrame::NetworkApplied {
                address: "192.168.64.253".into(),
            }),
        ));
        let outcome = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect("the guest answered");
        assert_eq!(
            outcome,
            Bootstrapped::Applied(Ipv4Addr::new(192, 168, 64, 253))
        );
        assert_eq!(
            guest.await.expect("the guest ran"),
            Some(ClientFrame::ConfigureNetwork(plan())),
            "the guest is told the whole plan, not just one address"
        );
    }

    #[tokio::test]
    async fn a_guest_that_refuses_gives_the_host_its_typed_reason() {
        let reason = BrokerExitReason::NoStaticAddress {
            offered: vec!["192.168.64.254".into()],
        };
        let (host, server) = tokio::io::duplex(4096);
        tokio::spawn(guest(server, Some(ServerFrame::Refused(reason.clone()))));
        assert_eq!(
            configure(host, &plan(), Duration::from_secs(5))
                .await
                .expect("a refusal is an answer, not a failure"),
            Bootstrapped::Refused(reason)
        );
    }

    #[tokio::test]
    async fn an_address_the_host_never_offered_is_not_taken_as_an_answer() {
        let (host, server) = tokio::io::duplex(4096);
        tokio::spawn(guest(
            server,
            Some(ServerFrame::NetworkApplied {
                address: "192.168.64.9".into(),
            }),
        ));
        let error = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect_err("an unoffered address is not this host's reservation");
        assert!(
            matches!(error, BootstrapError::Unoffered(a) if a == Ipv4Addr::new(192, 168, 64, 9)),
            "{error:?}"
        );
        assert!(error.to_string().contains("192.168.64.9"), "{error}");
    }

    #[tokio::test]
    async fn an_address_that_is_not_an_address_is_named_as_malformed() {
        let (host, server) = tokio::io::duplex(4096);
        tokio::spawn(guest(
            server,
            Some(ServerFrame::NetworkApplied {
                address: "not-an-address".into(),
            }),
        ));
        let error = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect_err("a malformed report is no report");
        assert!(matches!(error, BootstrapError::Malformed(_)), "{error:?}");
        assert!(error.to_string().contains("not-an-address"), "{error}");
    }

    #[tokio::test]
    async fn a_guest_that_hangs_up_before_answering_never_starts_a_workload() {
        let (host, server) = tokio::io::duplex(4096);
        tokio::spawn(guest(server, None));
        let error = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect_err("a closed channel is not an applied address");
        assert!(matches!(error, BootstrapError::Disconnected), "{error:?}");
        assert!(
            error.to_string().contains("without reporting an address"),
            "{error}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_guest_that_never_answers_gives_up_instead_of_holding_the_run_open() {
        let (host, server) = tokio::io::duplex(4096);
        let held = tokio::spawn(async move {
            let mut server = server;
            let mut sink = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_buf(&mut server, &mut sink).await;
            std::future::pending::<()>().await;
        });
        let error = configure(host, &plan(), Duration::from_secs(20))
            .await
            .expect_err("a silent guest must not hold the run open");
        assert!(
            matches!(error, BootstrapError::Timeout(d) if d == Duration::from_secs(20)),
            "{error:?}"
        );
        assert!(error.to_string().contains("20s"), "{error}");
        held.abort();
    }

    #[tokio::test]
    async fn a_frame_that_is_not_an_answer_to_the_plan_is_refused_by_name() {
        let (host, server) = tokio::io::duplex(4096);
        tokio::spawn(guest(
            server,
            Some(ServerFrame::StdoutBytes(b"hello".to_vec())),
        ));
        let error = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect_err("workload output is not an address");
        assert!(
            matches!(error, BootstrapError::Unexpected(what) if what == "workload output"),
            "{error:?}"
        );
        assert!(error.to_string().contains("before it reported"), "{error}");
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_written_to_fails_before_anything_waits() {
        let (host, server) = tokio::io::duplex(1);
        drop(server);
        let error = configure(host, &plan(), Duration::from_secs(5))
            .await
            .expect_err("a dead channel carries no plan");
        assert!(matches!(error, BootstrapError::Io(_)), "{error:?}");
        assert!(error.to_string().contains("plan"), "{error}");
    }
}
