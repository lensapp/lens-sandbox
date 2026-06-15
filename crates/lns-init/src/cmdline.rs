use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdlineParams {
    pub upper_dev: Option<String>,
    pub composefs_descriptor_dev: Option<String>,
    pub content_tag: Option<String>,
    pub composefs_descriptor_sha256: Option<String>,
    pub volumes: Vec<VolumeParam>,
    pub incomplete_volumes: Vec<usize>,
    pub binds: Vec<BindParam>,
    pub incomplete_binds: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeParam {
    pub dev: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindParam {
    pub tag: String,
    pub target: String,
    pub read_only: bool,
    pub dropped_paths: Vec<String>,
}

#[derive(Default)]
struct PartialBind {
    tag: Option<String>,
    target: Option<String>,
    read_only: bool,
    drops_count: Option<usize>,
    drops: BTreeMap<usize, String>,
}

enum BindOutcome {
    Ready(BindParam),
    Incomplete,
    Skip,
}

impl PartialBind {
    fn resolve(self) -> BindOutcome {
        let read_only = self.read_only;
        match (self.tag, self.target) {
            (None, None) => BindOutcome::Skip,
            (Some(_), None) | (None, Some(_)) => BindOutcome::Incomplete,
            (Some(_), Some(_)) if self.drops.len() != self.drops_count.unwrap_or(0) => {
                BindOutcome::Incomplete
            }
            (Some(tag), Some(target)) if target_within_root(&target) => {
                BindOutcome::Ready(BindParam {
                    tag,
                    target,
                    read_only,
                    dropped_paths: self.drops.into_values().collect(),
                })
            }
            (Some(_), Some(_)) => BindOutcome::Skip,
        }
    }
}

#[derive(Default)]
struct PartialVolume {
    dev: Option<String>,
    target: Option<String>,
    read_only: bool,
}

enum VolumeOutcome {
    Ready(VolumeParam),
    Incomplete,
    Skip,
}

impl PartialVolume {
    fn resolve(self) -> VolumeOutcome {
        let read_only = self.read_only;
        match (self.dev, self.target) {
            (None, None) => VolumeOutcome::Skip,
            (Some(_), None) | (None, Some(_)) => VolumeOutcome::Incomplete,
            (Some(dev), Some(target)) if target_within_root(&target) => {
                VolumeOutcome::Ready(VolumeParam {
                    dev,
                    target,
                    read_only,
                })
            }
            (Some(_), Some(_)) => VolumeOutcome::Skip,
        }
    }
}

fn target_within_root(target: &str) -> bool {
    target.starts_with('/') && !target.split('/').any(|seg| seg == "." || seg == "..")
}

impl CmdlineParams {
    pub fn parse(cmdline: &str) -> Self {
        let mut out = Self::default();
        let mut vols: BTreeMap<usize, PartialVolume> = BTreeMap::new();
        let mut binds: BTreeMap<usize, PartialBind> = BTreeMap::new();
        for tok in tokenize(cmdline) {
            let (key, value) = match tok.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            let value = strip_quotes(value);
            match key {
                "upper.dev" => out.upper_dev = Some(value.to_string()),
                "composefs.descriptor.dev" => {
                    out.composefs_descriptor_dev = Some(value.to_string())
                }
                "content.tag" => out.content_tag = Some(value.to_string()),
                "composefs.descriptor.sha256" => {
                    out.composefs_descriptor_sha256 = Some(value.to_string())
                }
                _ if key.starts_with("bind.") => parse_bind_key(key, value, &mut binds),
                _ => parse_volume_key(key, value, &mut vols),
            }
        }
        for (idx, partial) in vols {
            match partial.resolve() {
                VolumeOutcome::Ready(v) => out.volumes.push(v),
                VolumeOutcome::Incomplete => out.incomplete_volumes.push(idx),
                VolumeOutcome::Skip => {}
            }
        }
        for (idx, partial) in binds {
            match partial.resolve() {
                BindOutcome::Ready(b) => out.binds.push(b),
                BindOutcome::Incomplete => out.incomplete_binds.push(idx),
                BindOutcome::Skip => {}
            }
        }
        out
    }
}

fn parse_volume_key(key: &str, value: &str, vols: &mut BTreeMap<usize, PartialVolume>) {
    let Some((idx, field)) = key
        .strip_prefix("volume.")
        .and_then(|rest| rest.split_once('.'))
        .and_then(|(idx, field)| idx.parse::<usize>().ok().map(|i| (i, field)))
    else {
        return;
    };
    let entry = vols.entry(idx).or_default();
    match field {
        "dev" => entry.dev = Some(value.to_string()),
        "target" => entry.target = Some(value.to_string()),
        "ro" => entry.read_only = value == "1",
        _ => {}
    }
}

fn parse_bind_key(key: &str, value: &str, binds: &mut BTreeMap<usize, PartialBind>) {
    let Some((idx, field)) = key
        .strip_prefix("bind.")
        .and_then(|rest| rest.split_once('.'))
        .and_then(|(idx, field)| idx.parse::<usize>().ok().map(|i| (i, field)))
    else {
        return;
    };
    let entry = binds.entry(idx).or_default();
    match field {
        "tag" => entry.tag = Some(value.to_string()),
        "target" => entry.target = Some(value.to_string()),
        "ro" => entry.read_only = value == "1",
        "drops" => entry.drops_count = value.parse::<usize>().ok(),
        _ => {
            if let Some(j) = field
                .strip_prefix("drop.")
                .and_then(|j| j.parse::<usize>().ok())
            {
                entry.drops.insert(j, value.to_string());
            }
        }
    }
}

const INTERNAL_VAR_PREFIX: &str = "LENS_SANDBOX_";

pub fn sanitize_cmdline(cmdline: &str) -> String {
    tokenize(cmdline)
        .into_iter()
        .filter(|tok| !is_internal_var_token(tok))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_internal_var_token(tok: &str) -> bool {
    let key = tok.split_once('=').map(|(k, _)| k).unwrap_or(tok);
    key.starts_with(INTERNAL_VAR_PREFIX)
}

fn tokenize(cmdline: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let bytes = cmdline.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        let mut in_quotes = false;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'"' {
                in_quotes = !in_quotes;
                i += 1;
            } else if !in_quotes && b.is_ascii_whitespace() {
                break;
            } else {
                i += 1;
            }
        }
        if start < i {
            // SAFETY: boundaries are ASCII whitespace / quote bytes so the slice is still valid UTF-8.
            tokens.push(&cmdline[start..i]);
        }
    }
    tokens
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_phase0_keys() {
        let cmd = "console=hvc0 ip=dhcp \
                   upper.dev=/dev/vda \
                   composefs.descriptor.dev=/dev/vdb \
                   content.tag=lns-content \
                   composefs.descriptor.sha256=abc123";
        let p = CmdlineParams::parse(cmd);
        assert_eq!(p.upper_dev.as_deref(), Some("/dev/vda"));
        assert_eq!(p.composefs_descriptor_dev.as_deref(), Some("/dev/vdb"));
        assert_eq!(p.content_tag.as_deref(), Some("lns-content"));
        assert_eq!(p.composefs_descriptor_sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn unknown_tokens_are_ignored() {
        let p = CmdlineParams::parse("console=hvc0 quiet rootwait foo");
        assert_eq!(p, CmdlineParams::default());
    }

    #[test]
    fn repeated_key_takes_last_value() {
        let p = CmdlineParams::parse("upper.dev=/dev/sda upper.dev=/dev/vda");
        assert_eq!(p.upper_dev.as_deref(), Some("/dev/vda"));
    }

    #[test]
    fn quoted_value_preserves_spaces() {
        let p = CmdlineParams::parse(r#"content.tag="lns content""#);
        assert_eq!(p.content_tag.as_deref(), Some("lns content"));
    }

    #[test]
    fn empty_cmdline_yields_default() {
        let p = CmdlineParams::parse("");
        assert_eq!(p, CmdlineParams::default());
    }

    #[test]
    fn whitespace_only_cmdline_yields_default() {
        let p = CmdlineParams::parse("   \t   ");
        assert_eq!(p, CmdlineParams::default());
    }

    #[test]
    fn token_without_equals_is_skipped() {
        let p = CmdlineParams::parse("rdinit=/init upper.dev=/dev/vda");
        assert_eq!(p.upper_dev.as_deref(), Some("/dev/vda"));
    }

    #[test]
    fn parses_volume_keys_into_ordered_params() {
        let p = CmdlineParams::parse(
            "volume.1.dev=/dev/vdd volume.1.target=/srv volume.1.ro=1 \
             volume.0.dev=/dev/vdc volume.0.target=/data volume.0.ro=0",
        );
        assert_eq!(
            p.volumes,
            vec![
                VolumeParam {
                    dev: "/dev/vdc".into(),
                    target: "/data".into(),
                    read_only: false,
                },
                VolumeParam {
                    dev: "/dev/vdd".into(),
                    target: "/srv".into(),
                    read_only: true,
                },
            ]
        );
    }

    #[test]
    fn volume_missing_its_target_is_flagged_incomplete_not_silently_dropped() {
        let p = CmdlineParams::parse("volume.0.dev=/dev/vdc");
        assert!(p.volumes.is_empty());
        assert_eq!(
            p.incomplete_volumes,
            vec![0],
            "a truncated volume must be reported, not silently lost"
        );
    }

    #[test]
    fn volume_missing_its_dev_is_flagged_incomplete() {
        let p = CmdlineParams::parse("volume.2.target=/data");
        assert!(p.volumes.is_empty());
        assert_eq!(p.incomplete_volumes, vec![2]);
    }

    #[test]
    fn a_stray_volume_field_with_neither_dev_nor_target_is_skipped() {
        let p = CmdlineParams::parse("volume.0.ro=1");
        assert!(p.volumes.is_empty());
        assert!(
            p.incomplete_volumes.is_empty(),
            "a bare ro flag is not a truncated volume"
        );
    }

    #[test]
    fn a_complete_volume_is_not_flagged_incomplete() {
        let p = CmdlineParams::parse("volume.0.dev=/dev/vdc volume.0.target=/data");
        assert_eq!(p.volumes.len(), 1);
        assert!(p.incomplete_volumes.is_empty());
    }

    #[test]
    fn volume_ro_defaults_to_false_when_absent() {
        let p = CmdlineParams::parse("volume.0.dev=/dev/vdc volume.0.target=/data");
        assert_eq!(p.volumes.len(), 1);
        assert!(!p.volumes[0].read_only);
    }

    #[test]
    fn malformed_volume_index_is_ignored() {
        let p = CmdlineParams::parse("volume.x.dev=/dev/vdc volume.x.target=/data");
        assert!(p.volumes.is_empty());
    }

    #[test]
    fn unknown_volume_field_is_ignored() {
        let p = CmdlineParams::parse(
            "volume.0.dev=/dev/vdc volume.0.target=/data volume.0.bogus=whatever",
        );
        assert_eq!(p.volumes.len(), 1);
        assert_eq!(p.volumes[0].dev, "/dev/vdc");
        assert_eq!(p.volumes[0].target, "/data");
    }

    #[test]
    fn no_volume_keys_yields_empty_volumes() {
        let p = CmdlineParams::parse("upper.dev=/dev/vda");
        assert!(p.volumes.is_empty());
    }

    #[test]
    fn volume_with_a_dot_dot_target_is_dropped_so_the_mount_cannot_escape_newroot() {
        for target in [
            "/../../proc",
            "/data/../../etc",
            "/..",
            "/a/./b",
            "relative",
        ] {
            let p =
                CmdlineParams::parse(&format!("volume.0.dev=/dev/vdc volume.0.target={target}"));
            assert!(p.volumes.is_empty(), "{target} must be dropped");
        }
    }

    #[test]
    fn volume_with_a_dotted_filename_is_kept() {
        let p = CmdlineParams::parse("volume.0.dev=/dev/vdc volume.0.target=/srv/app.data");
        assert_eq!(p.volumes.len(), 1);
        assert_eq!(p.volumes[0].target, "/srv/app.data");
    }

    #[test]
    fn sanitize_strips_the_relay_token_so_the_workload_cmdline_never_carries_it() {
        let raw = "console=hvc0 upper.dev=/dev/vda LENS_SANDBOX_TOKEN=deadbeef quiet";
        let clean = sanitize_cmdline(raw);
        assert!(
            !clean.contains("deadbeef"),
            "token value leaked into sanitized cmdline: {clean}"
        );
        assert!(
            !clean.contains("LENS_SANDBOX_TOKEN"),
            "token key leaked into sanitized cmdline: {clean}"
        );
    }

    #[test]
    fn sanitize_drops_every_internal_var_but_keeps_boot_and_workload_tokens() {
        let raw = "console=hvc0 LENS_SANDBOX_WS_URL=vsock://host:1024/v1/sandbox \
                   LENS_SANDBOX_TOKEN=secret LENS_SANDBOX_USER=sandbox \
                   upper.dev=/dev/vda AGENT_COMMAND_B64=ZWNobyBoaQ== quiet";
        let clean = sanitize_cmdline(raw);
        let toks: Vec<&str> = clean.split_whitespace().collect();
        assert!(toks.contains(&"console=hvc0"));
        assert!(toks.contains(&"upper.dev=/dev/vda"));
        assert!(toks.contains(&"AGENT_COMMAND_B64=ZWNobyBoaQ=="));
        assert!(toks.contains(&"quiet"));
        assert!(
            !toks.iter().any(|t| t.starts_with("LENS_SANDBOX_")),
            "no LENS_SANDBOX_* token may survive sanitization: {toks:?}"
        );
    }

    #[test]
    fn sanitize_is_a_no_op_when_no_internal_vars_present() {
        let raw = "console=hvc0 upper.dev=/dev/vda quiet";
        assert_eq!(
            sanitize_cmdline(raw).split_whitespace().collect::<Vec<_>>(),
            ["console=hvc0", "upper.dev=/dev/vda", "quiet"]
        );
    }

    #[test]
    fn sanitize_drops_a_bare_internal_key_with_no_value() {
        let clean = sanitize_cmdline("console=hvc0 LENS_SANDBOX_DEBUG quiet");
        assert!(!clean.contains("LENS_SANDBOX_DEBUG"), "{clean}");
    }

    #[test]
    fn parses_bind_keys_with_ordered_dropped_paths() {
        let p = CmdlineParams::parse(
            "bind.0.tag=lns-bind-0 bind.0.target=/work bind.0.ro=0 \
             bind.0.drops=2 bind.0.drop.1=.npmrc bind.0.drop.0=.env",
        );
        assert_eq!(
            p.binds,
            vec![BindParam {
                tag: "lns-bind-0".into(),
                target: "/work".into(),
                read_only: false,
                dropped_paths: vec![".env".into(), ".npmrc".into()],
            }]
        );
        assert!(p.incomplete_binds.is_empty());
    }

    #[test]
    fn bind_ro_and_no_drops_parse_cleanly() {
        let p = CmdlineParams::parse(
            "bind.0.tag=lns-bind-0 bind.0.target=/cfg bind.0.ro=1 bind.0.drops=0",
        );
        assert_eq!(p.binds.len(), 1);
        assert!(p.binds[0].read_only);
        assert!(p.binds[0].dropped_paths.is_empty());
    }

    #[test]
    fn bind_missing_its_target_is_flagged_incomplete() {
        let p = CmdlineParams::parse("bind.0.tag=lns-bind-0");
        assert!(p.binds.is_empty());
        assert_eq!(p.incomplete_binds, vec![0]);
    }

    #[test]
    fn bind_with_fewer_drops_than_declared_is_incomplete_so_a_secret_is_never_silently_exposed() {
        let p = CmdlineParams::parse(
            "bind.0.tag=lns-bind-0 bind.0.target=/work bind.0.drops=2 bind.0.drop.0=.env",
        );
        assert!(p.binds.is_empty(), "a truncated drop list must not mount");
        assert_eq!(p.incomplete_binds, vec![0]);
    }

    #[test]
    fn bind_with_a_dot_dot_target_is_dropped_so_the_mount_cannot_escape_newroot() {
        let p = CmdlineParams::parse("bind.0.tag=lns-bind-0 bind.0.target=/../etc bind.0.drops=0");
        assert!(p.binds.is_empty());
    }

    #[test]
    fn malformed_bind_index_is_ignored() {
        let p = CmdlineParams::parse("bind.x.tag=lns-bind-0 bind.x.target=/work");
        assert!(p.binds.is_empty());
        assert!(p.incomplete_binds.is_empty());
    }

    #[test]
    fn no_bind_keys_yields_empty_binds() {
        let p = CmdlineParams::parse("upper.dev=/dev/vda");
        assert!(p.binds.is_empty());
    }

    #[test]
    fn a_stray_bind_field_with_neither_tag_nor_target_is_skipped() {
        let p = CmdlineParams::parse("bind.0.ro=1");
        assert!(p.binds.is_empty());
        assert!(
            p.incomplete_binds.is_empty(),
            "a bare ro flag is not a truncated bind"
        );
    }
}
