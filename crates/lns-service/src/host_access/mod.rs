use lns_ipc::{HostAccessGrant, SocketForward};

pub mod real;

/// Where the host stages the manifest inside the guest's runtime layer.
pub const MANIFEST_GUEST_PATH: &str = "/.lens/host-access";

/// Where a projected file is staged before the guest copies it into the run-as user's home. The workload never reads it here — `/.lens` belongs to the sandbox.
pub const STAGING_DIR: &str = "/.lens/host-access.d";

/// The host side of one guest→host socket forward: the guest dials `port`, and every accepted connection is proxied to `host_source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSocketSpec {
    pub id: String,
    pub host_source: String,
    pub port: u32,
}

/// The first port a forward may use. The relay owns 1024 and the guest port-forward listener 1030, so forwards start clear of both.
pub const FIRST_FORWARD_PORT: u32 = 1040;

/// One port per forward, assigned by declaration order, because a Vz listener is registered per port at VM-build time and the ids are known then.
pub fn plan(forwards: &[SocketForward]) -> Vec<HostSocketSpec> {
    forwards
        .iter()
        .enumerate()
        .map(|(index, forward)| HostSocketSpec {
            id: forward.id.clone(),
            host_source: forward.host_source.clone(),
            port: FIRST_FORWARD_PORT + index as u32,
        })
        .collect()
}

/// The instruction manifest the broker executes, one tab-separated line per step:
/// `dir <mode> <target>`, `file <mode> <staged> <target>`, `socket <port> <target>`.
/// A `~/` target is resolved in the guest against the run-as user's passwd home, which the host cannot spell. The host socket path never appears — the guest has no use for it.
pub fn manifest(grants: &[HostAccessGrant], plan: &[HostSocketSpec]) -> String {
    let mut out = String::new();
    for (index, grant) in grants.iter().enumerate() {
        for dir in &grant.dirs {
            out.push_str(&format!("dir\t{:o}\t{}\n", dir.mode, dir.target));
        }
        for (file_index, file) in grant.files.iter().enumerate() {
            out.push_str(&format!(
                "file\t{:o}\t{}\t{}\n",
                file.mode,
                staged_path(index, file_index),
                file.target
            ));
        }
    }
    // Matched by id, never by position: the grants and the forwards are two separately supplied request fields, so a reordered or partial list would otherwise cross-wire one capability's guest socket to another's host agent.
    for grant in grants {
        let Some(target) = &grant.socket_target else {
            continue;
        };
        if let Some(spec) = plan.iter().find(|spec| spec.id == grant.id) {
            out.push_str(&format!("socket\t{}\t{target}\n", spec.port));
        }
    }
    out
}

/// Indexed rather than named after the target, so a target containing a separator cannot decide where the staged bytes land.
pub fn staged_path(grant_index: usize, file_index: usize) -> String {
    format!("{STAGING_DIR}/{grant_index}.{file_index}")
}

/// The manifest plus one staged blob per projected file, as runtime-layer entries. Mode 0444 throughout: the guest copies them into the workload's home and owns the modes there, so nothing under `/.lens` needs to be writable.
pub fn staged_specs(
    grants: &[HostAccessGrant],
    plan: &[HostSocketSpec],
) -> Vec<crate::runtime_layer::RuntimeFileSpec> {
    if grants.is_empty() {
        return Vec::new();
    }
    let mut specs = vec![crate::runtime_layer::RuntimeFileSpec {
        guest_path: MANIFEST_GUEST_PATH.into(),
        mode: 0o444,
        source: crate::runtime_layer::RuntimeSource::Bytes(manifest(grants, plan).into_bytes()),
    }];
    for (index, grant) in grants.iter().enumerate() {
        for (file_index, file) in grant.files.iter().enumerate() {
            specs.push(crate::runtime_layer::RuntimeFileSpec {
                guest_path: staged_path(index, file_index),
                mode: 0o444,
                source: crate::runtime_layer::RuntimeSource::Bytes(file.contents.clone()),
            });
        }
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{HostAccessDir, HostAccessFile};

    fn forward(id: &str) -> SocketForward {
        SocketForward {
            id: id.into(),
            host_source: format!("/run/{id}.sock"),
            guest_target: "~/.gnupg/S.gpg-agent".into(),
        }
    }

    fn grant(id: &str) -> HostAccessGrant {
        HostAccessGrant {
            id: id.into(),
            socket_target: Some("~/.gnupg/S.gpg-agent".into()),
            dirs: vec![HostAccessDir {
                target: "~/.gnupg".into(),
                mode: 0o700,
            }],
            files: vec![HostAccessFile {
                target: "~/.gitconfig".into(),
                mode: 0o600,
                contents: b"[user]\n\temail = \"me@example.test\"\n".to_vec(),
            }],
        }
    }

    #[test]
    fn plan_gives_each_forward_its_own_port_in_declaration_order() {
        let planned = plan(&[forward("some-access"), forward("other-access")]);
        assert_eq!(planned[0].port, FIRST_FORWARD_PORT);
        assert_eq!(planned[1].port, FIRST_FORWARD_PORT + 1);
        assert_eq!(planned[1].host_source, "/run/other-access.sock");
    }

    #[test]
    fn planned_ports_never_collide_with_the_relay_or_the_guest_port_forward() {
        let planned = plan(&[forward("some-access")]);
        assert!(planned[0].port > lns_session::FORWARD_PORT);
        assert!(planned[0].port > lns_session::BROKER_PORT);
        assert!(planned[0].port > crate::relay::VSOCK_PORT);
    }

    #[test]
    fn plan_of_nothing_is_nothing() {
        assert!(plan(&[]).is_empty());
    }

    #[test]
    fn the_manifest_orders_the_home_before_the_files_and_the_socket_last() {
        let grants = [grant("some-access")];
        let rendered = manifest(&grants, &plan(&[forward("some-access")]));
        assert_eq!(
            rendered,
            format!(
                "dir\t700\t~/.gnupg\nfile\t600\t{STAGING_DIR}/0.0\t~/.gitconfig\nsocket\t{FIRST_FORWARD_PORT}\t~/.gnupg/S.gpg-agent\n"
            ),
            "the home must exist before a file lands in it, and the socket only once the rest is in place"
        );
    }

    #[test]
    fn the_manifest_never_names_the_host_socket() {
        let grants = [grant("some-access")];
        let rendered = manifest(&grants, &plan(&[forward("some-access")]));
        assert!(
            !rendered.contains("/run/some-access.sock"),
            "the host path is the one thing the guest must not learn: {rendered}"
        );
    }

    #[test]
    fn a_socket_line_is_matched_to_its_own_forward_by_id_not_by_position() {
        let mut without = grant("some-access");
        without.socket_target = None;
        let with = grant("other-access");
        let plan = plan(&[forward("other-access")]);
        let rendered = manifest(&[without, with], &plan);
        assert!(
            rendered.contains(&format!("socket\t{}\t~/.gnupg/S.gpg-agent\n", plan[0].port)),
            "the grant that asked for a socket must get the forward that carries its id:\n{rendered}"
        );
        assert_eq!(
            rendered.matches("socket\t").count(),
            1,
            "exactly one socket line:\n{rendered}"
        );
    }

    #[test]
    fn a_socket_target_with_no_matching_forward_renders_no_line() {
        let rendered = manifest(&[grant("some-access")], &plan(&[forward("other-access")]));
        assert!(
            !rendered.contains("socket\t"),
            "a grant whose forward is absent must not borrow another's port:\n{rendered}"
        );
    }

    #[test]
    fn a_grant_with_no_socket_still_projects_its_files() {
        let mut g = grant("some-access");
        g.socket_target = None;
        let rendered = manifest(&[g], &[]);
        assert!(rendered.contains("~/.gitconfig"), "{rendered}");
        assert!(!rendered.contains("socket\t"), "{rendered}");
    }

    #[test]
    fn the_manifest_of_nothing_is_empty_so_the_broker_does_nothing() {
        assert!(manifest(&[], &[]).is_empty());
    }

    #[test]
    fn staged_specs_carry_the_manifest_and_one_blob_per_projected_file() {
        let grants = [grant("some-access")];
        let specs = staged_specs(&grants, &plan(&[forward("some-access")]));
        let paths: Vec<&str> = specs.iter().map(|s| s.guest_path.as_str()).collect();
        assert_eq!(paths, [MANIFEST_GUEST_PATH, &format!("{STAGING_DIR}/0.0")]);
        assert!(specs.iter().all(|s| s.mode == 0o444));
    }

    #[test]
    fn staged_specs_of_no_grant_stage_nothing_not_even_an_empty_manifest() {
        assert!(staged_specs(&[], &[]).is_empty());
    }

    #[test]
    fn a_staged_path_is_indexed_so_a_target_cannot_decide_where_bytes_land() {
        assert_eq!(staged_path(1, 2), format!("{STAGING_DIR}/1.2"));
    }
}
