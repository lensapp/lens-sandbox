use super::{Ctx, Image, Outcome, guest_output, missing_command, requires, timed};
use crate::result::CaseResult;

pub fn lease_and_resolver(ctx: &Ctx) -> CaseResult {
    timed("lease-and-resolver", |case| {
        let script = format!(
            "{}ip -4 addr show eth0\ncat /etc/resolv.conf\n",
            requires(&["ip", "cat"])
        );
        let output = ctx.guest(Image::Alpine, "parity-lease", &script)?;
        guest_output(case, &output);
        if let Some(reason) = missing_command(&output) {
            return Ok(Outcome::Skip(reason));
        }
        if !output.ok() {
            return Ok(Outcome::Fail(format!(
                "the guest did not report its lease: {}",
                output.combined().trim()
            )));
        }

        let combined = output.combined();
        let Some((address, prefix)) = parse_inet(&combined) else {
            return Ok(Outcome::Fail("no `inet` address on eth0".to_string()));
        };
        case.record("address", address);
        case.record("prefix", prefix as i64);

        let Some(nameserver) = parse_nameserver(&combined) else {
            return Ok(Outcome::Fail(
                "the guest resolv.conf names no nameserver".to_string(),
            ));
        };
        case.record("nameserver", nameserver);
        Ok(Outcome::Pass)
    })
}

fn parse_inet(output: &str) -> Option<(String, u8)> {
    let token = output
        .split_whitespace()
        .skip_while(|token| *token != "inet")
        .nth(1)?;
    let (address, prefix) = token.split_once('/')?;
    Some((address.to_string(), prefix.parse().ok()?))
}

fn parse_nameserver(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("nameserver "))
        .map(|rest| rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_and_its_prefix_come_from_the_guests_own_report() {
        let output = "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\n    inet 192.168.127.2/24 brd 192.168.127.255 scope global eth0\n";
        assert_eq!(parse_inet(output), Some(("192.168.127.2".to_string(), 24)));
        assert_eq!(parse_inet("2: eth0: <UP> mtu 1500\n"), None);
    }

    #[test]
    fn the_resolver_comes_from_the_guests_own_resolv_conf() {
        assert_eq!(
            parse_nameserver("search lan\nnameserver 192.168.127.1\n"),
            Some("192.168.127.1".to_string())
        );
        assert_eq!(parse_nameserver("search lan\n"), None);
    }
}
