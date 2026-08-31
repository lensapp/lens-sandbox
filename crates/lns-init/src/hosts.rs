#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const LOOPBACK_NAMES: &str = "127.0.0.1\tlocalhost\n::1\tlocalhost\n";
const HOSTS_MODE: u32 = 0o644;

pub fn seed_if_absent(newroot: &str) -> io::Result<()> {
    let etc = Path::new(newroot).join("etc");
    let hosts = etc.join("hosts");
    if hosts.symlink_metadata().is_ok() {
        return Ok(());
    }
    write_loopback_names(&etc, &hosts)
        .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", hosts.display())))
}

fn write_loopback_names(etc: &Path, hosts: &Path) -> io::Result<()> {
    std::fs::create_dir_all(etc)?;
    std::fs::write(hosts, LOOPBACK_NAMES)?;
    std::fs::set_permissions(hosts, std::fs::Permissions::from_mode(HOSTS_MODE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn newroot() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn an_image_that_ships_no_hosts_file_still_resolves_localhost() {
        let root = newroot();

        seed_if_absent(root.path().to_str().unwrap()).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.path().join("etc/hosts")).unwrap(),
            "127.0.0.1\tlocalhost\n::1\tlocalhost\n"
        );
    }

    #[test]
    fn the_seeded_file_is_readable_by_the_confined_workload() {
        let root = newroot();

        seed_if_absent(root.path().to_str().unwrap()).unwrap();

        let mode = std::fs::metadata(root.path().join("etc/hosts"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn a_hosts_file_the_image_ships_survives_byte_for_byte() {
        let root = newroot();
        std::fs::create_dir(root.path().join("etc")).unwrap();
        let shipped = "10.0.0.1\tregistry.internal\n";
        std::fs::write(root.path().join("etc/hosts"), shipped).unwrap();

        seed_if_absent(root.path().to_str().unwrap()).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.path().join("etc/hosts")).unwrap(),
            shipped
        );
    }

    #[test]
    fn a_hosts_symlink_the_image_ships_is_left_where_it_points() {
        let root = newroot();
        std::fs::create_dir(root.path().join("etc")).unwrap();
        std::os::unix::fs::symlink("/run/hosts", root.path().join("etc/hosts")).unwrap();

        seed_if_absent(root.path().to_str().unwrap()).unwrap();

        assert_eq!(
            std::fs::read_link(root.path().join("etc/hosts")).unwrap(),
            Path::new("/run/hosts"),
            "the image decides where its hosts file lives, dangling or not"
        );
    }

    #[test]
    fn a_rootfs_that_refuses_the_write_names_the_path_it_refused() {
        let root = newroot();
        std::fs::write(root.path().join("etc"), b"not a directory").unwrap();

        let err = seed_if_absent(root.path().to_str().unwrap()).unwrap_err();

        assert!(
            err.to_string().contains("etc/hosts"),
            "an operator needs the path that refused, not a bare errno: {err}"
        );
    }
}
