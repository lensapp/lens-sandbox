use super::{Ctx, Image, Outcome, field, guest_output, missing_command, requires, timed};
use crate::fixtures::Role;
use crate::result::CaseResult;

pub fn loopback_witness(ctx: &Ctx) -> CaseResult {
    timed("loopback-witness", |case| {
        let port = ctx.port(Role::Witness);
        let gateway_host = format!("{}.254", ctx.guest_subnet);
        let before = ctx.fixtures.report().witness_accepts;
        let script = format!(
            "{prelude}probe() {{\n  body=$(curl -sS --max-time 5 \"$2\" 2>/dev/null || true)\n  echo \"$1-EXIT=$?\"\n  echo \"$1-BODY=[$body]\"\n}}\nprobe LOOPBACK http://127.0.0.1:{port}/\nprobe SUBNET http://{gateway_host}:{port}/\n",
            prelude = requires(&["curl"]),
        );
        let output = ctx.guest(Image::Curl, "parity-loopback", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let combined = output.combined();
        for probe in ["LOOPBACK", "SUBNET"] {
            if let Some(exit) = field(&combined, &format!("{probe}-EXIT=")) {
                case.record(&format!("{}_curl_exit", probe.to_lowercase()), exit);
            }
            let answered = combined.contains(&format!("{probe}-BODY=[REACHED!]"));
            case.record(&format!("{}_answered", probe.to_lowercase()), answered);
        }

        let accepts = ctx.fixtures.report().witness_accepts - before;
        case.record("witness_accepts", accepts);
        if let Some(expected) = ctx.expectation("loopback-witness") {
            case.record("expectation", expected);
        }

        if accepts > 0 {
            return Ok(Outcome::Fail(format!(
                "the witness on 127.0.0.1:{port} accepted {accepts} connection(s) from the guest"
            )));
        }
        Ok(Outcome::Pass)
    })
}
