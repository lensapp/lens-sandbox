mod constants;
mod dir;
mod extents;
mod format;
mod journal;
mod layout;
mod plan;
mod writer;

mod real;
pub use plan::Plan;
pub use real::provision;
pub use writer::write_ext4;

pub const DEFAULT_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

fn provision_image(
    root: &std::path::Path,
    run_id: &str,
    uuid: [u8; 16],
    mkfs_time: u32,
    write: impl FnOnce(&Plan, &std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<std::path::PathBuf> {
    let run_dir = crate::cache::run_dir(root, run_id);
    std::fs::create_dir_all(&run_dir)?;
    let path = run_dir.join("upper.img");
    if path.exists() {
        return Ok(path);
    }
    let plan = Plan::new(DEFAULT_SIZE_BYTES, uuid, "lns-upper", mkfs_time)?;
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
        let path = provision_image(root.path(), "aa07", [0xCD; 16], 99, |_plan, p| {
            wrote.set(true);
            std::fs::write(p, b"img").map_err(Into::into)
        })
        .unwrap();
        assert!(wrote.get(), "writer was invoked");
        assert_eq!(
            path,
            root.path().join("runs").join("aa07").join("upper.img")
        );
        assert!(path.exists());
    }

    #[test]
    fn provision_image_keeps_an_existing_writable_layer_untouched() {
        let root = tempfile::TempDir::new().unwrap();
        let existing = root.path().join("runs").join("aa07");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("upper.img"), b"a stopped run's data").unwrap();
        let path = provision_image(root.path(), "aa07", [0; 16], 0, |_, _| {
            panic!("reformatted")
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"a stopped run's data");
    }

    #[test]
    fn provision_image_propagates_writer_failure() {
        let root = tempfile::TempDir::new().unwrap();
        let err = provision_image(root.path(), "aa01", [0; 16], 0, |_, _| {
            Err(anyhow::anyhow!("write boom"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("write boom"));
    }

    #[test]
    fn provision_image_errors_when_run_dir_cannot_be_created() {
        // A regular file as the cache root makes create_dir_all under it fail.
        let file = tempfile::NamedTempFile::new().unwrap();
        let err = provision_image(file.path(), "aa01", [0; 16], 0, |_, _| Ok(())).unwrap_err();
        assert!(
            err.downcast_ref::<std::io::Error>().is_some(),
            "io error from create_dir_all: {err}"
        );
    }
}
