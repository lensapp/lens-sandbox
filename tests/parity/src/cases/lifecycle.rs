use super::{
    Ctx, Image, Outcome, guest_output, missing_command, poll, record_transfer, requires, timed,
};
use crate::fixtures::Role;
use crate::result::CaseResult;
use std::time::Duration;

const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const DESCRIPTOR_TIMEOUT: Duration = Duration::from_secs(10);
const CYCLES: usize = 20;
const CYCLE_GROWTH_ALLOWED: i64 = 3;

pub fn kill_mid_transfer(ctx: &Ctx) -> CaseResult {
    timed("kill-mid-transfer", |case| {
        let baseline = ctx.open_fds();
        if let Some(count) = baseline {
            case.record("descriptors_before", count);
        }

        let mark = ctx.conn_mark();
        let script = format!(
            "{}dd if=/dev/zero bs=1M count=1024 2>/dev/null | nc {} {}\n",
            requires(&["dd", "nc"]),
            ctx.lan(),
            ctx.port(Role::Sink)
        );
        let started = ctx.guest_detached(Image::Alpine, "parity-kill", &script)?;
        guest_output(case, &started);
        if let Some(reason) = missing_command(&started) {
            return Ok(Outcome::Skip(reason));
        }
        if !started.ok() {
            return Ok(Outcome::Fail(format!(
                "the detached guest did not start: {}",
                started.combined().trim()
            )));
        }

        let opened = ctx.await_connection(Role::Sink, mark, BOOT_TIMEOUT);
        if opened.is_none() {
            let _ = ctx.kill("parity-kill");
            let _ = ctx.remove("parity-kill");
            return Ok(Outcome::Fail(
                "the detached guest never reached the sink".to_string(),
            ));
        }

        std::thread::sleep(Duration::from_secs(3));
        let killed = ctx.kill("parity-kill")?;
        case.record("kill_exit_code", killed.code as i64);

        let closed = ctx.await_closed(Role::Sink, mark, CLOSE_TIMEOUT);
        let Some(record) = closed else {
            let _ = ctx.remove("parity-kill");
            return Ok(Outcome::Fail(
                "the fixture's connection stayed open more than 5 s after `lns kill`".to_string(),
            ));
        };
        record_transfer(case, "sink", &record);

        let bytes_at_close = record.bytes_in;
        std::thread::sleep(Duration::from_secs(2));
        let after = ctx
            .new_connection(Role::Sink, mark)
            .map(|record| record.bytes_in)
            .unwrap_or(bytes_at_close);
        case.record("sink_bytes_after_close", after);

        let recovered = poll(DESCRIPTOR_TIMEOUT, || {
            let now = ctx.open_fds()?;
            (baseline.is_some_and(|before| now <= before)).then_some(now)
        });
        if let Some(count) = recovered {
            case.record("descriptors_after", count);
        } else if let Some(count) = ctx.open_fds() {
            case.record("descriptors_after", count);
        }

        let _ = ctx.remove("parity-kill");

        if after != bytes_at_close {
            return Ok(Outcome::Fail(format!(
                "the fixture read {} more bytes after the connection closed",
                after - bytes_at_close
            )));
        }
        match (baseline, recovered) {
            (Some(_), None) => Ok(Outcome::Fail(
                "the service's descriptor count never returned to its pre-case value within 10 s"
                    .to_string(),
            )),
            _ => Ok(Outcome::Pass),
        }
    })
}

pub fn create_destroy_20(ctx: &Ctx) -> CaseResult {
    timed("create-destroy-20", |case| {
        let baseline = ctx.open_fds();
        if let Some(count) = baseline {
            case.record("descriptors_before", count);
        }

        let script = format!(
            "{}sleep 30 | nc {} {}\n",
            requires(&["nc", "sleep"]),
            ctx.lan(),
            ctx.port(Role::Echo)
        );

        let mut completed = 0;
        for cycle in 0..CYCLES {
            let name = format!("parity-cycle-{cycle}");
            let mark = ctx.conn_mark();
            let started = ctx.guest_detached(Image::Alpine, &name, &script)?;
            if cycle == 0 {
                guest_output(case, &started);
                if let Some(reason) = missing_command(&started) {
                    return Ok(Outcome::Skip(reason));
                }
            }
            if !started.ok() {
                case.record("failed_cycle", cycle as i64);
                let _ = ctx.remove(&name);
                return Ok(Outcome::Fail(format!(
                    "cycle {cycle} did not start: {}",
                    started.combined().trim()
                )));
            }
            let reached = ctx
                .await_connection(Role::Echo, mark, BOOT_TIMEOUT)
                .is_some();
            let _ = ctx.kill(&name);
            let _ = ctx.remove(&name);
            if !reached {
                case.record("failed_cycle", cycle as i64);
                return Ok(Outcome::Fail(format!(
                    "cycle {cycle} never held a connection to the fixture"
                )));
            }
            completed += 1;
        }
        case.record("cycles", completed as i64);

        let after = ctx.open_fds();
        if let Some(count) = after {
            case.record("descriptors_after", count);
        }
        let (Some(before), Some(after)) = (baseline, after) else {
            return Ok(Outcome::Skip(
                "this host reports no descriptor count for the service".to_string(),
            ));
        };
        let delta = after as i64 - before as i64;
        case.record("descriptor_delta", delta);
        if delta > CYCLE_GROWTH_ALLOWED {
            return Ok(Outcome::Blocked(format!(
                "the service holds {delta} more descriptors after {CYCLES} runs; the known per-run growth is lens-sandbox issue #427"
            )));
        }
        Ok(Outcome::Pass)
    })
}
