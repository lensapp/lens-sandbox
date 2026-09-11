use crate::result::HostFacts;
use std::process::Command;

pub fn facts() -> HostFacts {
    HostFacts {
        os: std::env::consts::OS.to_string(),
        os_version: os_version(),
        arch: std::env::consts::ARCH.to_string(),
        dns_scope_count: dns_scope_count(),
    }
}

fn os_version() -> String {
    let command = if cfg!(target_os = "macos") {
        Command::new("sw_vers").arg("-productVersion").output()
    } else {
        Command::new("uname").arg("-r").output()
    };
    command
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn dns_scope_count() -> Option<u32> {
    let output = Command::new("scutil").arg("--dns").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(count_resolvers(&String::from_utf8_lossy(&output.stdout)))
}

pub fn count_resolvers(output: &str) -> u32 {
    output
        .lines()
        .filter(|line| line.trim_start().starts_with("resolver #"))
        .count() as u32
}

pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resolver_scopes_are_counted_as_scutil_prints_them() {
        let output = "DNS configuration\n\nresolver #1\n  nameserver[0] : 1.1.1.1\n\nresolver #2\n  domain : corp.example\n\nDNS configuration (for scoped queries)\n\nresolver #1\n  nameserver[0] : 10.0.0.1\n";
        assert_eq!(count_resolvers(output), 3);
        assert_eq!(count_resolvers(""), 0);
    }

    #[test]
    fn the_host_facts_name_this_machine() {
        let facts = facts();
        assert_eq!(facts.os, std::env::consts::OS);
        assert_eq!(facts.arch, std::env::consts::ARCH);
        assert!(!facts.os_version.is_empty());
        assert_eq!(is_macos(), cfg!(target_os = "macos"));
        if !is_macos() {
            assert_eq!(
                facts.dns_scope_count, None,
                "only macOS reports DNS resolver scopes"
            );
        }
    }
}
