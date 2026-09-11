use super::{
    Ctx, Image, Outcome, field, guest_output, missing_command, parse_dd_bytes, record_transfer,
    requires, throughput_mib_s, timed,
};
use crate::fixtures::Role;
use crate::fixtures::pattern::{MIB, ZEROS_10MIB_SHA256, ZEROS_100MIB_SHA256, pattern_sha256};
use crate::result::CaseResult;
use std::time::Duration;

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(180);
const NO_HALF_CLOSE: &str = "PARITY_NO_HALF_CLOSE";

pub fn upload_100m(ctx: &Ctx) -> CaseResult {
    timed("upload-100m", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}dd if=/dev/zero bs=1M count=100 2>/dev/null | nc {} {}\n",
            requires(&["dd", "nc"]),
            ctx.lan(),
            ctx.port(Role::Sink)
        );
        let output = ctx.guest(Image::Alpine, "parity-upload", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let Some(record) = ctx.await_closed(Role::Sink, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the sink never saw the guest's upload close".to_string(),
            ));
        };
        record_transfer(case, "sink", &record);
        if let Some(rate) = throughput_mib_s(record.bytes_in, &record) {
            case.record("throughput_mib_s", rate);
        }

        let expected = 100 * MIB;
        if record.bytes_in != expected {
            return Ok(Outcome::Fail(format!(
                "the sink read {} bytes, not {expected}",
                record.bytes_in
            )));
        }
        if record.sha_in.as_deref() != Some(ZEROS_100MIB_SHA256) {
            return Ok(Outcome::Fail(
                "the sink's hash is not the hash of 100 MiB of zeros".to_string(),
            ));
        }
        Ok(Outcome::Pass)
    })
}

pub fn download_100m(ctx: &Ctx) -> CaseResult {
    timed("download-100m", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}nc {} {} | sha256sum\n",
            requires(&["nc", "sha256sum"]),
            ctx.lan(),
            ctx.port(Role::Source)
        );
        let output = ctx.guest(Image::Alpine, "parity-download", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let Some(record) = ctx.await_closed(Role::Source, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the source never saw the guest's download close".to_string(),
            ));
        };
        record_transfer(case, "source", &record);
        if let Some(rate) = throughput_mib_s(record.bytes_out, &record) {
            case.record("throughput_mib_s", rate);
        }

        let host_sha = pattern_sha256(100 * MIB);
        let Some(guest_sha) = first_token(&output.stdout) else {
            return Ok(Outcome::Fail(
                "the guest printed no hash of what it read".to_string(),
            ));
        };
        case.record("guest_sha256", guest_sha.clone());
        case.record("host_sha256", host_sha.clone());
        if guest_sha != host_sha {
            return Ok(Outcome::Fail(
                "the guest's hash differs from the pattern the source sent".to_string(),
            ));
        }
        Ok(Outcome::Pass)
    })
}

pub fn bidirectional_100m(ctx: &Ctx) -> CaseResult {
    timed("bidirectional-100m", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}dd if=/dev/zero bs=1M count=100 2>/dev/null | nc {} {} | dd bs=1M 2>/tmp/rx.err | sha256sum\ncat /tmp/rx.err\n",
            requires(&["dd", "nc", "sha256sum"]),
            ctx.lan(),
            ctx.port(Role::Bidirectional)
        );
        let output = ctx.guest(Image::Alpine, "parity-bidir", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let Some(record) = ctx.await_closed(Role::Bidirectional, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the fixture never saw the two-way transfer close".to_string(),
            ));
        };
        record_transfer(case, "fixture", &record);

        let expected = 100 * MIB;
        case.record("guest_sent_bytes", expected);
        if let Some(received) = parse_dd_bytes(&output.combined()) {
            case.record("guest_received_bytes", received);
            if received != expected {
                return Ok(Outcome::Fail(format!(
                    "the guest read {received} bytes, not {expected}"
                )));
            }
        }

        let host_sha = pattern_sha256(expected);
        let guest_sha = first_token(&output.stdout).unwrap_or_default();
        case.record("guest_sha256", guest_sha.clone());
        case.record("host_sha256", host_sha.clone());
        if record.bytes_in != expected || record.bytes_out != expected {
            return Ok(Outcome::Fail(format!(
                "the fixture counted {} in and {} out, not {expected} each",
                record.bytes_in, record.bytes_out
            )));
        }
        if record.sha_in.as_deref() != Some(ZEROS_100MIB_SHA256) || guest_sha != host_sha {
            return Ok(Outcome::Fail(
                "the two-way transfer changed the bytes on at least one side".to_string(),
            ));
        }
        Ok(Outcome::Pass)
    })
}

pub fn guest_half_close(ctx: &Ctx) -> CaseResult {
    timed("guest-half-close", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}if nc --help 2>&1 | grep -q -- '-N'; then FLAG='-N'; elif nc --help 2>&1 | grep -q -- '-q'; then FLAG='-q 1'; else echo {NO_HALF_CLOSE}; exit 43; fi\necho PARITY_FLAG=$FLAG\ndd if=/dev/zero bs=1M count=10 2>/dev/null | nc $FLAG {} {} | wc -c\n",
            requires(&["dd", "nc", "wc"]),
            ctx.lan(),
            ctx.port(Role::HalfCloseReply)
        );
        let output = ctx.guest(Image::Alpine, "parity-guest-half-close", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }
        if output.combined().contains(NO_HALF_CLOSE) {
            return Ok(Outcome::Skip(
                "the image's nc has neither -N nor -q, so the guest cannot half-close".to_string(),
            ));
        }
        if let Some(flag) = field(&output.combined(), "PARITY_FLAG=") {
            case.record("guest_half_close_flag", flag);
        }

        let Some(record) = ctx.await_closed(Role::HalfCloseReply, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the fixture never saw the half-closed connection close".to_string(),
            ));
        };
        record_transfer(case, "fixture", &record);

        let received = last_number(&output.stdout);
        if let Some(bytes) = received {
            case.record("guest_reply_bytes", bytes);
        }
        if !record.saw_eof || record.bytes_in != 10 * MIB {
            return Ok(Outcome::Fail(format!(
                "the fixture read {} bytes and {} the guest's EOF",
                record.bytes_in,
                if record.saw_eof { "saw" } else { "never saw" }
            )));
        }
        if record.sha_in.as_deref() != Some(ZEROS_10MIB_SHA256) {
            return Ok(Outcome::Fail(
                "the 10 MiB the fixture read is not what the guest sent".to_string(),
            ));
        }
        if received != Some(MIB) {
            return Ok(Outcome::Fail(format!(
                "the guest read {received:?} of the fixture's 1 MiB reply"
            )));
        }
        Ok(Outcome::Pass)
    })
}

pub fn host_half_close(ctx: &Ctx) -> CaseResult {
    timed("host-half-close", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}dd if=/dev/zero bs=1M count=10 2>/dev/null | nc {} {} | sha256sum\n",
            requires(&["dd", "nc", "sha256sum"]),
            ctx.lan(),
            ctx.port(Role::HostHalfClose)
        );
        let output = ctx.guest(Image::Alpine, "parity-host-half-close", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let Some(record) = ctx.await_closed(Role::HostHalfClose, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the fixture never saw the connection close after its own half close".to_string(),
            ));
        };
        record_transfer(case, "fixture", &record);

        let host_sha = pattern_sha256(10 * MIB);
        let guest_sha = first_token(&output.stdout).unwrap_or_default();
        case.record("guest_sha256", guest_sha.clone());
        case.record("host_sha256", host_sha.clone());
        if guest_sha != host_sha {
            return Ok(Outcome::Fail(
                "the guest did not read the whole 10 MiB the fixture sent before its half close"
                    .to_string(),
            ));
        }
        if record.bytes_in != 10 * MIB || record.sha_in.as_deref() != Some(ZEROS_10MIB_SHA256) {
            return Ok(Outcome::Fail(format!(
                "the fixture read {} bytes from the guest after its own half close",
                record.bytes_in
            )));
        }
        Ok(Outcome::Pass)
    })
}

pub fn reset_mid_transfer(ctx: &Ctx) -> CaseResult {
    timed("reset-mid-transfer", |case| {
        let mark = ctx.conn_mark();
        let script = format!(
            "{}start=$(date +%s)\nnc {} {} > /dev/null\ncode=$?\nend=$(date +%s)\necho PARITY_EXIT=$code\necho PARITY_ELAPSED=$((end-start))\n",
            requires(&["nc", "date"]),
            ctx.lan(),
            ctx.port(Role::Reset)
        );
        let output = ctx.guest(Image::Alpine, "parity-reset", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let Some(record) = ctx.await_closed(Role::Reset, mark, TRANSFER_TIMEOUT) else {
            return Ok(Outcome::Fail(
                "the fixture never reset the connection".to_string(),
            ));
        };
        record_transfer(case, "fixture", &record);
        case.record("fixture_reset_sent", record.reset_sent);

        let combined = output.combined();
        let exit: Option<i64> = field(&combined, "PARITY_EXIT=").and_then(|v| v.parse().ok());
        let elapsed: Option<i64> = field(&combined, "PARITY_ELAPSED=").and_then(|v| v.parse().ok());
        if let Some(code) = exit {
            case.record("guest_nc_exit_code", code);
        }
        if let Some(seconds) = elapsed {
            case.record("guest_seconds_after_reset", seconds);
        }

        if record.bytes_out != 5 * MIB || !record.reset_sent {
            return Ok(Outcome::Fail(format!(
                "the fixture committed {} bytes before the reset",
                record.bytes_out
            )));
        }
        match (exit, elapsed) {
            (Some(0), _) => Ok(Outcome::Fail(
                "the guest's transfer exited 0 through a reset".to_string(),
            )),
            (Some(_), Some(seconds)) if seconds > 5 => Ok(Outcome::Fail(format!(
                "the guest took {seconds}s to notice the reset"
            ))),
            (Some(_), _) => Ok(Outcome::Pass),
            (None, _) => Ok(Outcome::Fail(
                "the guest reported no exit code for its transfer".to_string(),
            )),
        }
    })
}

fn first_token(output: &str) -> Option<String> {
    output.split_whitespace().next().map(str::to_string)
}

fn last_number(output: &str) -> Option<u64> {
    output
        .split_whitespace()
        .filter_map(|token| token.parse().ok())
        .next_back()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guests_hash_is_the_first_token_sha256sum_prints() {
        assert_eq!(first_token("6c3fc0 -\n"), Some("6c3fc0".to_string()));
        assert_eq!(first_token("  \n"), None);
    }

    #[test]
    fn the_reply_size_is_the_count_wc_printed_last() {
        assert_eq!(last_number("PARITY_FLAG=-N\n1048576\n"), Some(1_048_576));
        assert_eq!(last_number("nothing counted\n"), None);
    }
}
