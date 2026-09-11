use std::future::Future;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Denied,
    ApprovalUnavailable,
    ApprovalTimeout,
    ResolverUnavailable,
    ResolverTimeout,
}

pub async fn resolve<T, F: Future<Output = Result<T, Refusal>>>(
    approve: impl Future<Output = Result<bool, Refusal>>,
    upstream: impl FnOnce() -> F,
) -> Result<T, Refusal> {
    let allowed = tokio::time::timeout(Duration::from_secs(60), approve)
        .await
        .map_err(|_| Refusal::ApprovalTimeout)??;
    if !allowed {
        return Err(Refusal::Denied);
    }
    tokio::time::timeout(Duration::from_secs(2), upstream())
        .await
        .map_err(|_| Refusal::ResolverTimeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tokio::sync::oneshot;

    #[tokio::test(start_paused = true)]
    async fn lost_or_expired_approval_never_starts_resolution() {
        for expires in [false, true] {
            let calls = Cell::new(0);
            let result = resolve(
                async {
                    if expires {
                        std::future::pending::<()>().await;
                    }
                    Err(Refusal::ApprovalUnavailable)
                },
                || async {
                    calls.set(calls.get() + 1);
                    Ok(())
                },
            )
            .await;
            assert_eq!(
                result,
                Err(if expires {
                    Refusal::ApprovalTimeout
                } else {
                    Refusal::ApprovalUnavailable
                })
            );
            assert_eq!(calls.get(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn resolver_failures_do_not_retry_through_another_resolver() {
        for expires in [false, true] {
            let calls = Cell::new(0);
            let result = resolve(async { Ok(true) }, || async {
                calls.set(calls.get() + 1);
                if expires {
                    std::future::pending::<()>().await;
                }
                Err::<(), _>(Refusal::ResolverUnavailable)
            })
            .await;
            assert_eq!(
                result,
                Err(if expires {
                    Refusal::ResolverTimeout
                } else {
                    Refusal::ResolverUnavailable
                })
            );
            assert_eq!(calls.get(), 1);
        }
    }

    #[tokio::test]
    async fn dns_permission_does_not_authorize_a_connection() {
        let (tx, _requests) = tokio::sync::mpsc::unbounded_channel();
        let gate = crate::Gate::new(tx).unwrap();
        assert_eq!(
            resolve(async { Ok(true) }, || async { Ok(()) }).await,
            Ok(())
        );
        let decision = gate
            .engine
            .authorize_egress(&openshell_supervisor_network::opa::NetworkInput {
                host: "example.com".into(),
                port: 443,
                binary_path: "/usr/bin/curl".into(),
                binary_sha256: String::new(),
                ancestors: vec![],
                cmdline_paths: vec![],
            })
            .unwrap();
        assert!(
            matches!(decision.action, openshell_supervisor_network::opa::NetworkAction::Deny { reason } if reason == "lns:ask")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn denial_never_calls_the_external_resolver() {
        let calls = Cell::new(0);
        let result = resolve(async { Ok(false) }, || async {
            calls.set(calls.get() + 1);
            Ok("address")
        })
        .await;
        assert_eq!(calls.get(), 0, "DNS escaped before permission");
        assert_eq!(result, Err(Refusal::Denied));
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_approval_does_not_spend_the_upstream_timeout() {
        let calls = Cell::new(0);
        let (answer, decision) = oneshot::channel();
        let lookup = resolve(
            async { decision.await.map_err(|_| Refusal::ApprovalUnavailable) },
            || async {
                calls.set(calls.get() + 1);
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok("address")
            },
        );
        tokio::pin!(lookup);
        tokio::select! {
            result = &mut lookup => panic!("lookup completed before approval: {result:?}"),
            () = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
        assert_eq!(calls.get(), 0, "pending approval must not send DNS");
        answer.send(true).unwrap();
        assert_eq!(lookup.await, Ok("address"));
        assert_eq!(calls.get(), 1);
    }
}
