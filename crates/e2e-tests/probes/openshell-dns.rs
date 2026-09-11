use lns_openshell_spike::dns::{Refusal, resolve};
use openshell_supervisor_network::run::resolve_dns_upstream;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

type Error = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    tokio::time::timeout(Duration::from_secs(25), probe()).await??;
    Ok(())
}

async fn probe() -> Result<(), Error> {
    for (allow, client_timeout, succeeds) in
        [(true, 2, false), (false, 15, false), (true, 15, true)]
    {
        scenario(allow, client_timeout, succeeds).await?;
    }
    println!("openshell-dns-boundary-ok");
    Ok(())
}

async fn scenario(allow: bool, client_timeout: u64, succeeds: bool) -> Result<(), Error> {
    let external = UdpSocket::bind("127.0.0.1:0").await?;
    let external_address = external.local_addr()?;
    let observed = Arc::new(AtomicUsize::new(0));
    let packets = observed.clone();
    let external_task = tokio::spawn(async move {
        let mut packet = [0u8; 8192];
        let (length, peer) = external.recv_from(&mut packet).await?;
        packets.fetch_add(1, Ordering::SeqCst);
        let mut response = packet[..length].to_vec();
        response[2] = 0x81;
        response[3] = 0x80;
        response[6] = 0;
        response[7] = 1;
        response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 203, 0, 113, 7]);
        external.send_to(&response, peer).await?;
        Ok::<_, std::io::Error>(())
    });
    let local = UdpSocket::bind("127.0.0.1:0").await?;
    let local_address = local.local_addr()?;
    let (answer, decision) = oneshot::channel();
    let (received, ready) = oneshot::channel();
    let proxy = tokio::spawn(async move {
        let mut packet = [0u8; 8192];
        let (length, peer) = local
            .recv_from(&mut packet)
            .await
            .map_err(|_| Refusal::ResolverUnavailable)?;
        let _ = received.send(());
        let result = resolve(
            async { decision.await.map_err(|_| Refusal::ApprovalUnavailable) },
            || async {
                let upstream = UdpSocket::bind("127.0.0.1:0")
                    .await
                    .map_err(|_| Refusal::ResolverUnavailable)?;
                upstream
                    .connect(external_address)
                    .await
                    .map_err(|_| Refusal::ResolverUnavailable)?;
                upstream
                    .send(&packet[..length])
                    .await
                    .map_err(|_| Refusal::ResolverUnavailable)?;
                let mut reply = vec![0; 8192];
                let size = upstream
                    .recv(&mut reply)
                    .await
                    .map_err(|_| Refusal::ResolverUnavailable)?;
                reply.truncate(size);
                Ok(reply)
            },
        )
        .await;
        let reply = match result {
            Ok(reply) => reply,
            Err(Refusal::Denied) => {
                let mut reply = packet[..length].to_vec();
                reply[2] = 0x81;
                reply[3] = 0x85;
                reply
            }
            Err(error) => return Err(error),
        };
        local
            .send_to(&reply, peer)
            .await
            .map_err(|_| Refusal::ResolverUnavailable)?;
        Ok(())
    });
    let client = tokio::spawn(resolve_dns_upstream(
        local_address,
        "demo.example",
        false,
        Duration::from_secs(client_timeout),
    ));
    ready.await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        observed.load(Ordering::SeqCst),
        0,
        "external DNS before approval"
    );
    if client_timeout == 2 {
        assert!(
            client.is_finished(),
            "expected stock timeout before human approval"
        );
        assert!(client.await?.is_err());
        drop(answer);
        assert_eq!(proxy.await?, Err(Refusal::ApprovalUnavailable));
        external_task.abort();
    } else {
        assert!(
            !client.is_finished(),
            "approval-aware timeout expired early"
        );
        answer
            .send(allow)
            .map_err(|_| "approval receiver disappeared")?;
        let result = client.await?;
        assert_eq!(result.is_ok(), succeeds, "{result:?}");
        assert_eq!(proxy.await?, Ok(()));
        if allow {
            assert_eq!(
                result.map_err(|e| e.to_string())?,
                vec!["203.0.113.7".parse::<std::net::IpAddr>()?]
            );
            external_task.await??;
        } else {
            external_task.abort();
        }
    }
    assert_eq!(observed.load(Ordering::SeqCst), usize::from(succeeds));
    println!(
        "allow={allow} client_timeout={client_timeout}s external_queries={}",
        observed.load(Ordering::SeqCst)
    );
    Ok(())
}
