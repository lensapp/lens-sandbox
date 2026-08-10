#[cfg(target_os = "linux")]
mod real;

#[cfg(target_os = "linux")]
pub use real::apply;

/// The instruction manifest the host stages for this run.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const MANIFEST_PATH: &str = "/.lens/host-access";

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Create `target` with `mode`, owned by the run-as user.
    Dir { target: String, mode: u32 },
    /// Copy the bytes staged at `staged` to `target` with `mode`, owned by the run-as user.
    File {
        staged: String,
        target: String,
        mode: u32,
    },
    /// Create a unix socket at `target` whose every connection is relayed to the host over vsock `port`.
    Socket { target: String, port: u32 },
}

/// The home every `~/` target resolves against: a leading `home` line carries what the author declared, which the supervisor also gives the workload, and otherwise the run-as user's passwd entry.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn effective_home<'a>(text: &'a str, passwd_home: &'a str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix("home\t"))
        .map(str::trim)
        .filter(|home| home.starts_with('/') && !home.split('/').any(|seg| seg == ".."))
        .unwrap_or(passwd_home)
}

/// A malformed line is skipped rather than failing the boot: one bad instruction must not cost the workload its whole session.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_manifest(text: &str, passwd_home: &str) -> Vec<Step> {
    let home = effective_home(text, passwd_home);
    text.lines()
        .filter_map(|line| parse_step(line, home))
        .collect()
}

fn parse_step(line: &str, home: &str) -> Option<Step> {
    let mut fields = line.split('\t');
    match fields.next()? {
        "dir" => {
            let mode = parse_mode(fields.next()?)?;
            let target = resolve_target(fields.next()?, home)?;
            Some(Step::Dir { target, mode })
        }
        "file" => {
            let mode = parse_mode(fields.next()?)?;
            let staged = fields.next()?.to_string();
            let target = resolve_target(fields.next()?, home)?;
            // Only the host's own staging area may be a source; anything else would let a manifest line copy an arbitrary guest file into the workload's home.
            (staged.starts_with("/.lens/")).then_some(Step::File {
                staged,
                target,
                mode,
            })
        }
        "socket" => {
            let port: u32 = fields.next()?.trim().parse().ok()?;
            let target = resolve_target(fields.next()?, home)?;
            Some(Step::Socket { target, port })
        }
        _ => None,
    }
}

fn parse_mode(field: &str) -> Option<u32> {
    u32::from_str_radix(field.trim(), 8).ok()
}

/// `~/` resolves against the run-as user's passwd home, which only the guest knows — the host cannot spell this path.
fn resolve_target(target: &str, home: &str) -> Option<String> {
    let target = target.trim();
    let resolved = match target.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None if target.starts_with('/') => target.to_string(),
        None => return None,
    };
    if resolved.split('/').any(|seg| seg == "..") {
        return None;
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_home_line_is_what_every_target_resolves_against() {
        let parsed = parse_manifest(
            "home\t/home/sandbox\nsocket\t1040\t~/.gnupg/S.gpg-agent\n",
            "/root",
        );
        assert_eq!(
            parsed,
            [Step::Socket {
                target: "/home/sandbox/.gnupg/S.gpg-agent".into(),
                port: 1040,
            }],
            "the projection must land where the workload's HOME points, not where passwd does"
        );
    }

    #[test]
    fn the_passwd_home_is_used_when_the_manifest_declares_none() {
        assert_eq!(effective_home("dir\t700\t~/x\n", "/var/www"), "/var/www");
    }

    #[test]
    fn a_home_line_that_could_escape_is_ignored_in_favour_of_passwd() {
        for bad in ["relative", "/home/../etc", ""] {
            assert_eq!(
                effective_home(&format!("home\t{bad}\n"), "/root"),
                "/root",
                "{bad:?} must not steer the projection"
            );
        }
    }

    #[test]
    fn a_home_line_cannot_smuggle_a_second_instruction_past_the_host() {
        // Defence in depth: the host refuses a forgeable home, and a line the guest cannot resolve is still dropped rather than executed against passwd.
        let forged = "home\t/home/sandbox\ndir\t777\t~/../../etc\n";
        assert!(
            parse_manifest(forged, "/root").is_empty(),
            "a target escaping the home must not become a step"
        );
    }

    #[test]
    fn the_home_line_itself_is_not_an_instruction() {
        assert!(parse_manifest("home\t/home/sandbox\n", "/root").is_empty());
    }

    #[test]
    fn a_home_relative_target_resolves_against_the_run_as_users_home() {
        let parsed = parse_manifest("socket\t1040\t~/.gnupg/S.gpg-agent\n", "/home/sandbox");
        assert_eq!(
            parsed,
            [Step::Socket {
                target: "/home/sandbox/.gnupg/S.gpg-agent".into(),
                port: 1040,
            }]
        );
    }

    #[test]
    fn a_trailing_slash_on_the_home_does_not_double_up() {
        let parsed = parse_manifest("dir\t700\t~/.gnupg\n", "/root/");
        assert_eq!(
            parsed,
            [Step::Dir {
                target: "/root/.gnupg".into(),
                mode: 0o700,
            }]
        );
    }

    #[test]
    fn a_mode_is_read_as_octal_the_way_the_host_wrote_it() {
        let parsed = parse_manifest(
            "dir\t700\t/x\nfile\t600\t/.lens/host-access.d/0.0\t/y\n",
            "/h",
        );
        assert_eq!(
            parsed,
            [
                Step::Dir {
                    target: "/x".into(),
                    mode: 0o700,
                },
                Step::File {
                    staged: "/.lens/host-access.d/0.0".into(),
                    target: "/y".into(),
                    mode: 0o600,
                },
            ]
        );
    }

    #[test]
    fn every_step_is_kept_in_order_so_the_home_exists_before_a_file_lands_in_it() {
        let parsed = parse_manifest(
            "dir\t700\t~/.gnupg\nfile\t600\t/.lens/host-access.d/0.0\t~/.gnupg/pubring.kbx\nsocket\t1040\t~/.gnupg/S.gpg-agent\n",
            "/home/sandbox",
        );
        assert_eq!(parsed.len(), 3);
        assert!(matches!(parsed[0], Step::Dir { .. }));
        assert!(matches!(parsed[1], Step::File { .. }));
        assert!(matches!(parsed[2], Step::Socket { .. }));
    }

    #[test]
    fn a_malformed_line_is_skipped_without_costing_the_other_steps() {
        let parsed = parse_manifest(
            "no-tab\nbogus\t700\t/x\ndir\tnotoctal\t/x\nsocket\tnotanumber\t~/x\n\ndir\t700\t~/good\n",
            "/home/sandbox",
        );
        assert_eq!(
            parsed,
            [Step::Dir {
                target: "/home/sandbox/good".into(),
                mode: 0o700,
            }]
        );
    }

    #[test]
    fn a_staged_source_outside_the_sandbox_namespace_is_refused() {
        let parsed = parse_manifest("file\t600\t/etc/shadow\t~/.gitconfig\n", "/home/sandbox");
        assert!(
            parsed.is_empty(),
            "only the host's own staging area may be a source: {parsed:?}"
        );
    }

    #[test]
    fn a_target_escaping_the_home_is_refused() {
        for bad in [
            "~/../escape",
            "~/ok/../bad",
            "/run/../etc/passwd",
            "relative",
        ] {
            let parsed = parse_manifest(&format!("dir\t700\t{bad}\n"), "/home/sandbox");
            assert!(parsed.is_empty(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn an_empty_manifest_asks_for_nothing() {
        assert!(parse_manifest("", "/home/sandbox").is_empty());
    }
}
