use super::{
    Ctx, Image, Outcome, PREFLIGHT, field, guest_output, missing_command, record_transfer,
    requires, timed,
};
use crate::fixtures::Role;
use crate::result::CaseResult;

pub const EXCHANGE_SECONDS: u64 = 10;
const PAYLOAD: &str = "0123456789abcdef";

pub fn fixture_reachable(ctx: &Ctx) -> CaseResult {
    timed(PREFLIGHT, |case| {
        let mark = ctx.conn_mark();
        let lan = ctx.lan();
        let port = ctx.port(Role::Echo);
        let sent = PAYLOAD.len() as i64;
        let script = format!(
            "{prelude}back=$(printf '%s' {PAYLOAD} | nc -w {EXCHANGE_SECONDS} {lan} {port} | wc -c)\necho PARITY_ECHOED=$back\n",
            prelude = requires(&["nc", "wc", "printf"]),
        );
        let output = ctx.guest(Image::Alpine, "parity-preflight", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let echoed: Option<i64> =
            field(&output.combined(), "PARITY_ECHOED=").and_then(|value| value.trim().parse().ok());
        case.record("sent_bytes", sent);
        case.record("echoed_bytes", echoed.unwrap_or(-1));
        if let Some(record) = ctx.new_connection(Role::Echo, mark) {
            record_transfer(case, "echo", &record);
        }

        if echoed == Some(sent) {
            return Ok(Outcome::Pass);
        }
        Ok(Outcome::Fail(unreachable_message(
            echoed,
            sent,
            &format!("{lan}:{port}"),
        )))
    })
}

fn unreachable_message(echoed: Option<i64>, sent: i64, destination: &str) -> String {
    let read = match echoed {
        Some(bytes) => format!("read {bytes} back"),
        None => "reported nothing".to_string(),
    };
    format!(
        "the guest sent {sent} bytes to {destination} over raw TCP and {read} within {EXCHANGE_SECONDS}s; the definition's egress.tcp must decide that destination, or the guest's proxy holds the stream"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_stream_names_the_destination_and_the_rule_that_would_carry_it() {
        let message = unreachable_message(Some(0), 16, "192.168.1.49:47206");

        assert!(message.contains("192.168.1.49:47206"), "{message}");
        assert!(message.contains("egress.tcp"), "{message}");
        assert!(message.contains("read 0 back"), "{message}");
    }

    #[test]
    fn a_guest_that_printed_no_count_says_so_rather_than_naming_a_number() {
        let message = unreachable_message(None, 16, "192.168.1.49:47206");
        assert!(message.contains("reported nothing"), "{message}");
    }
}
