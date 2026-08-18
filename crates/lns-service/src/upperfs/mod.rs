mod constants;
mod dir;
mod extents;
mod format;
pub mod grow;
mod journal;
mod layout;
mod plan;
mod writer;

mod real;
pub use plan::Plan;
pub use real::provision;
pub use writer::{grow_ext4, write_ext4};

pub const DEFAULT_SIZE_BYTES: u64 = lns_artifact::resources::DEFAULT_VM_SIZE.disk_bytes;

fn provision_image(
    root: &std::path::Path,
    run_id: &str,
    uuid: [u8; 16],
    mkfs_time: u32,
    size_bytes: u64,
    write: impl FnOnce(&Plan, &std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<std::path::PathBuf> {
    let run_dir = crate::cache::run_dir(root, run_id);
    std::fs::create_dir_all(&run_dir)?;
    let path = run_dir.join("upper.img");
    // A stopped sandbox keeps its writable layer, so starting it again finds this file and must not reformat it.
    if path.exists() {
        return Ok(path);
    }
    let plan = Plan::new(size_bytes, uuid, "lns-upper", mkfs_time)?;
    write(&plan, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn provision_image_creates_run_dir_and_writes_through() {
        let root = tempfile::TempDir::new().unwrap();
        let wrote = Cell::new(false);
        let path = provision_image(
            root.path(),
            "aa07",
            [0xCD; 16],
            99,
            DEFAULT_SIZE_BYTES,
            |_plan, p| {
                wrote.set(true);
                std::fs::write(p, b"img").map_err(Into::into)
            },
        )
        .unwrap();
        assert!(wrote.get(), "writer was invoked");
        assert_eq!(
            path,
            root.path().join("runs").join("aa07").join("upper.img")
        );
        assert!(path.exists());
    }

    #[test]
    fn provision_image_sizes_the_disk_the_run_asked_for() {
        let root = tempfile::TempDir::new().unwrap();
        let sized = Cell::new(0u64);
        provision_image(root.path(), "aa08", [0; 16], 0, 40 << 30, |plan, _| {
            sized.set(plan.layout.image_size_bytes());
            Ok(())
        })
        .unwrap();
        assert_eq!(sized.get(), 40 << 30);
    }

    #[test]
    fn provision_image_keeps_an_existing_writable_layer_untouched() {
        let root = tempfile::TempDir::new().unwrap();
        let existing = root.path().join("runs").join("aa07");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("upper.img"), b"a stopped sandbox's data").unwrap();
        let path = provision_image(
            root.path(),
            "aa07",
            [0; 16],
            0,
            DEFAULT_SIZE_BYTES,
            |_, _| panic!("a preserved writable layer must never be reformatted"),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a stopped sandbox's data");
    }

    #[test]
    fn provision_image_propagates_writer_failure() {
        let root = tempfile::TempDir::new().unwrap();
        let err = provision_image(
            root.path(),
            "aa01",
            [0; 16],
            0,
            DEFAULT_SIZE_BYTES,
            |_, _| Err(anyhow::anyhow!("write boom")),
        )
        .unwrap_err();
        assert!(err.to_string().contains("write boom"));
    }

    #[test]
    fn provision_image_errors_when_run_dir_cannot_be_created() {
        // A regular file as the cache root makes create_dir_all under it fail.
        let file = tempfile::NamedTempFile::new().unwrap();
        let err = provision_image(
            file.path(),
            "aa01",
            [0; 16],
            0,
            DEFAULT_SIZE_BYTES,
            write_ext4,
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<std::io::Error>().is_some(),
            "io error from create_dir_all: {err}"
        );
    }
}
