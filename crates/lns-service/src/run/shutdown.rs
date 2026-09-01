use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::forward::ForwardGuard;
use crate::log;

pub(crate) async fn shutdown_after_session(
    forwards: ForwardGuard,
    grace: Duration,
    mut vm_task: JoinHandle<Result<()>>,
) -> Result<()> {
    drop(forwards);
    let sleep = tokio::time::sleep(grace);
    tokio::pin!(sleep);
    tokio::select! {
        _ = &mut sleep => {
            log::debug!("vm did not stop within grace period; proceeding");
        }
        r = &mut vm_task => {
            r??;
        }
    }
    Ok(())
}

pub(crate) async fn publish_exit_after_quiesce(
    run_id: &str,
    code: i32,
    shutdown: impl std::future::Future<Output = Result<()>>,
) -> Result<i32> {
    let quiesce = shutdown.await;
    crate::run_registry::set_exit_code(run_id, code);
    quiesce?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::{ForwardError, ForwardSpec, PortForwarder, establish, plan};
    use lns_ipc::{PortPublish, Protocol};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TimestampForwarder {
        bound: Mutex<Vec<SocketAddr>>,
        unbind_instants: Mutex<Vec<tokio::time::Instant>>,
    }

    impl TimestampForwarder {
        fn unbind_instants(&self) -> Vec<tokio::time::Instant> {
            self.unbind_instants.lock().unwrap().clone()
        }
    }

    impl PortForwarder for TimestampForwarder {
        fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError> {
            self.bound.lock().unwrap().push(spec.bind);
            Ok(())
        }

        fn unbind(&self, _bind: SocketAddr) {
            self.unbind_instants
                .lock()
                .unwrap()
                .push(tokio::time::Instant::now());
        }
    }

    fn pp(host_port: u16) -> PortPublish {
        PortPublish {
            host_ip: "127.0.0.1".parse().unwrap(),
            host_port,
            container_port: host_port,
            protocol: Protocol::Tcp,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ports_released_before_grace_sleep() {
        let fake = Arc::new(TimestampForwarder::default());
        let specs = plan(&[pp(3003)]);
        let guard = establish(fake.clone(), &specs).unwrap();
        let t0 = tokio::time::Instant::now();

        // VM task that never completes — guarantees the grace timer wins and
        // exercises the "vm did not stop within grace period" branch.
        let vm_task = tokio::spawn(std::future::pending::<Result<()>>());

        shutdown_after_session(guard, Duration::from_secs(2), vm_task)
            .await
            .unwrap();

        let instants = fake.unbind_instants();
        assert_eq!(instants.len(), 1);
        assert_eq!(
            instants[0], t0,
            "unbind must happen at T=0, before the 2s grace sleep"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ports_released_when_vm_exits_within_grace() {
        let fake = Arc::new(TimestampForwarder::default());
        let specs = plan(&[pp(8080)]);
        let guard = establish(fake.clone(), &specs).unwrap();
        let t0 = tokio::time::Instant::now();

        let vm_task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        });

        shutdown_after_session(guard, Duration::from_secs(2), vm_task)
            .await
            .unwrap();

        let instants = fake.unbind_instants();
        assert_eq!(instants.len(), 1);
        assert_eq!(instants[0], t0, "unbind still happens at T=0");
        assert_eq!(
            tokio::time::Instant::now().duration_since(t0),
            Duration::from_millis(100),
            "helper returns when VM exits, not after full grace"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ports_released_even_when_vm_task_panics() {
        let fake = Arc::new(TimestampForwarder::default());
        let specs = plan(&[pp(9090)]);
        let guard = establish(fake.clone(), &specs).unwrap();

        let vm_task = tokio::spawn(async { panic!("vm exploded") });
        tokio::task::yield_now().await;

        let result = shutdown_after_session(guard, Duration::from_secs(2), vm_task).await;

        assert!(result.is_err());
        assert_eq!(
            fake.unbind_instants().len(),
            1,
            "ports still released on panic"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env, global_runs)]
    async fn exit_is_published_only_after_the_vm_quiesce() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let seen = Arc::new(Mutex::new(None));
        let seen_in_shutdown = seen.clone();
        let shutdown_id = id.clone();
        let code = publish_exit_after_quiesce(&id, 7, async move {
            *seen_in_shutdown.lock().unwrap() = crate::run_registry::status(&shutdown_id);
            Ok(())
        })
        .await;
        let status_after = crate::run_registry::status(&id);
        crate::run_registry::deregister(&id);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(lns_ipc::RunStatus::Running),
            "rm and prune must not see Exited while the VM still holds the writable layer"
        );
        assert_eq!(status_after, Some(lns_ipc::RunStatus::Exited { code: 7 }));
        assert_eq!(code.unwrap(), 7);
    }

    #[tokio::test]
    #[serial_test::serial(env, global_runs)]
    async fn a_failed_quiesce_still_publishes_the_exit() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let result =
            publish_exit_after_quiesce(&id, 3, async { anyhow::bail!("vm never stopped") }).await;
        let status_after = crate::run_registry::status(&id);
        crate::run_registry::deregister(&id);
        assert!(result.is_err());
        assert_eq!(
            status_after,
            Some(lns_ipc::RunStatus::Exited { code: 3 }),
            "a failed quiesce must not leave the run stuck as Running forever"
        );
    }
}
