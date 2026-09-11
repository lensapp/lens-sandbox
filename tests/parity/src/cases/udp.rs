use super::{Ctx, Image, Outcome, field, guest_output, missing_command, requires, timed};
use crate::fixtures::Role;
use crate::result::CaseResult;

const NO_EMPTY_SENDER: &str = "PARITY_NO_EMPTY_SENDER";

pub fn udp_echo(ctx: &Ctx) -> CaseResult {
    timed("udp-echo", |case| {
        let before = ctx.fixtures.report().datagrams.len();
        let lan = ctx.lan();
        let port = ctx.port(Role::UdpEcho);
        let script = format!(
            "{prelude}echoed=$(printf '%0100d' 0 | nc -u -w 2 {lan} {port} | wc -c)\necho PARITY_ECHOED=$echoed\nif command -v python3 >/dev/null 2>&1; then\n  python3 -c \"import socket,sys;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.settimeout(3);s.sendto(b'',('{lan}',{port}));d,_=s.recvfrom(64);print('PARITY_EMPTY_BACK=%d'%len(d))\" || echo PARITY_EMPTY_BACK=timeout\nelif command -v perl >/dev/null 2>&1; then\n  perl -e 'use Socket; socket(S,PF_INET,SOCK_DGRAM,getprotobyname(\"udp\")); my $a=sockaddr_in({port},inet_aton(\"{lan}\")); send(S,\"\",0,$a); $SIG{{ALRM}}=sub{{print \"PARITY_EMPTY_BACK=timeout\\n\"; exit 0}}; alarm(3); my $b; recv(S,$b,64,0); print \"PARITY_EMPTY_BACK=\".length($b).\"\\n\"'\nelse\n  echo {NO_EMPTY_SENDER}\nfi\n",
            prelude = requires(&["nc", "wc", "printf"]),
        );
        let output = ctx.guest(Image::Alpine, "parity-udp", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }

        let combined = output.combined();
        let echoed: Option<i64> = field(&combined, "PARITY_ECHOED=").and_then(|v| v.parse().ok());
        if let Some(bytes) = echoed {
            case.record("hundred_byte_echo_bytes", bytes);
        }

        let datagrams = ctx.fixtures.report().datagrams;
        let seen: Vec<usize> = datagrams.iter().skip(before).map(|d| d.len).collect();
        case.record("fixture_datagrams", seen.len() as i64);
        case.record(
            "fixture_empty_datagrams",
            seen.iter().filter(|len| **len == 0).count() as i64,
        );

        if echoed != Some(100) {
            return Ok(Outcome::Fail(format!(
                "the guest read {echoed:?} bytes back from the 100-byte datagram"
            )));
        }

        if combined.contains(NO_EMPTY_SENDER) {
            return Ok(Outcome::Skip(
                "the guest image has no tool that sends a zero-length datagram; the empty-datagram check needs a custom image"
                    .to_string(),
            ));
        }
        let empty_back = field(&combined, "PARITY_EMPTY_BACK=");
        if let Some(value) = &empty_back {
            case.record("empty_datagram_reply", value.clone());
        }
        match empty_back.as_deref() {
            Some("0") => Ok(Outcome::Pass),
            Some(other) => Ok(Outcome::Fail(format!(
                "the empty datagram came back as {other}"
            ))),
            None => Ok(Outcome::Fail(
                "the guest reported nothing about the empty datagram".to_string(),
            )),
        }
    })
}
