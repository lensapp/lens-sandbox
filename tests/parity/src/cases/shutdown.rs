use super::{Ctx, Image, Outcome, guest_output, missing_command, poll, requires, timed};
use crate::fixtures::Role;
use crate::lns::pid_alive;
use crate::result::CaseResult;
use std::time::Duration;

const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const GUESTS: usize = 2;

pub fn service_stop_with_guests(ctx: &Ctx) -> CaseResult {
    timed("service-stop-with-guests", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}dd if=/dev/zero bs=1M count=1024 2>/dev/null | nc {} {}\n",
            requires(&["dd", "nc"]),
            ctx.lan(),
            ctx.port(Role::Sink)
        );

        let names: Vec<String> = (0..GUESTS).map(|i| format!("parity-stop-{i}")).collect();
        for name in &names {
            let started = ctx.guest_detached(Image::Alpine, name, &script)?;
            if let Some(reason) = missing_command(&started) {
                return Ok(Outcome::Skip(reason));
            }
            if !started.ok() {
                guest_output(case, &started);
                for name in &names {
                    let _ = ctx.remove(name);
                }
                return Ok(Outcome::Fail(format!(
                    "the detached guest {name} did not start: {}",
                    started.combined().trim()
                )));
            }
        }

        let sink_port = ctx.port(Role::Sink);
        let open = poll(BOOT_TIMEOUT, || {
            let report = ctx.fixtures.report();
            let ids: Vec<u64> = report
                .connections
                .iter()
                .filter(|record| record.port == sink_port && record.id > mark)
                .map(|record| record.id)
                .collect();
            (ids.len() >= GUESTS).then_some(ids)
        });
        let Some(ids) = open else {
            for name in &names {
                let _ = ctx.kill(name);
                let _ = ctx.remove(name);
            }
            return Ok(Outcome::Fail(
                "the two detached guests never both reached the sink".to_string(),
            ));
        };
        case.record("guests_mid_transfer", ids.len() as i64);

        let stopped = ctx.lns.run(&["service", "stop"])?;
        case.record("service_stop_exit_code", stopped.code as i64);

        let closed = poll(SHUTDOWN_TIMEOUT, || {
            let report = ctx.fixtures.report();
            let all_closed = ids.iter().all(|id| {
                report
                    .connections
                    .iter()
                    .any(|record| record.id == *id && record.closed_ms.is_some())
            });
            all_closed.then_some(())
        });
        case.record("connections_closed_within_10s", closed.is_some());

        let gone = match ctx.service_pid {
            Some(pid) => poll(SHUTDOWN_TIMEOUT, || (!pid_alive(pid)).then_some(())).is_some(),
            None => false,
        };
        case.record("service_pid_gone", gone);

        if closed.is_none() {
            return Ok(Outcome::Fail(
                "a guest's connection outlived `lns service stop` by more than 10 s".to_string(),
            ));
        }
        if !gone {
            return Ok(Outcome::Fail(
                "the private service is still alive 10 s after `lns service stop`".to_string(),
            ));
        }
        Ok(Outcome::Pass)
    })
}
