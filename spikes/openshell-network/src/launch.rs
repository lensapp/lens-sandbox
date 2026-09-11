use std::collections::HashMap;
use std::io;
use tokio::process::Command;

#[cfg(target_os = "linux")]
pub mod real;
#[cfg(target_os = "linux")]
mod root;

pub const ROOT_CAPABILITIES: u64 = 0xfb;

pub fn validate_identity(_uid: u32, isolated: bool) -> io::Result<()> {
    if !isolated {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "workload requires an isolated network namespace",
        ));
    }
    Ok(())
}

pub fn command(
    program: &str,
    args: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(
            env.iter()
                .filter(|(key, _)| !key.starts_with("LENS_") && !key.starts_with("OPENSHELL_")),
        )
        .kill_on_drop(true);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_keeps_explicit_identity_and_never_receives_internal_environment() {
        let env = HashMap::from([
            ("HOME".into(), "/home/node".into()),
            ("USER".into(), "node".into()),
            ("PATH".into(), "/tools/bin:/usr/bin".into()),
            ("LENS_SANDBOX_TOKEN".into(), "fixture-only".into()),
            ("OPENSHELL_INTERNAL".into(), "fixture-only".into()),
        ]);
        let cmd = command("/bin/sh", &["-e".into(), "step".into()], "/tmp", &env);
        let actual = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap()))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            actual,
            HashMap::from([
                ("HOME", "/home/node"),
                ("USER", "node"),
                ("PATH", "/tools/bin:/usr/bin")
            ])
        );
        assert_eq!(
            cmd.as_std().get_current_dir(),
            Some(std::path::Path::new("/tmp"))
        );
    }

    #[test]
    fn every_workload_requires_isolation_regardless_of_identity() {
        assert!(validate_identity(0, false).is_err());
        assert!(validate_identity(0, true).is_ok());
        assert!(validate_identity(1000, false).is_err());
        assert!(validate_identity(1000, true).is_ok());
    }

    #[test]
    fn root_retains_package_install_capabilities_without_network_or_namespace_admin() {
        assert_eq!(ROOT_CAPABILITIES, 0xfb);
        for capability in [12, 13, 16, 19, 21, 27, 31] {
            assert_eq!(ROOT_CAPABILITIES & (1 << capability), 0);
        }
    }
}
