mod constants;
mod dir;
mod extents;
mod format;
mod layout;
mod plan;
mod writer;

mod real;
pub use plan::Plan;
pub use real::provision;
pub use writer::write_ext4;

pub const DEFAULT_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn provision_image(
    root: &std::path::Path,
    run_id: &str,
    uuid: [u8; 16],
    mkfs_time: u32,
    write: impl FnOnce(&Plan, &std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<std::path::PathBuf> {
    let run_dir = root.join("runs").join(run_id);
    std::fs::create_dir_all(&run_dir)?;
    let path = run_dir.join("upper.img");
    let plan = Plan::new(DEFAULT_SIZE_BYTES, uuid, "lns-upper", mkfs_time);
    write(&plan, &path)?;
    Ok(path)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn provision_image_from_template(
    root: &std::path::Path,
    run_id: &str,
    template: &std::path::Path,
    clone: impl FnOnce(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
    fallback: impl FnOnce(&std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<std::path::PathBuf> {
    let run_dir = root.join("runs").join(run_id);
    std::fs::create_dir_all(&run_dir)?;
    let path = run_dir.join("upper.img");
    if let Err(e) = clone(template, &path) {
        crate::log::debug!("cloning upper template failed ({e:#}); writing the image directly");
        fallback(&path)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    #[test]
    fn template_clone_success_skips_the_direct_writer() {
        let root = tempfile::TempDir::new().unwrap();
        let cloned = Cell::new(false);
        let path = provision_image_from_template(
            root.path(),
            "aa08",
            std::path::Path::new("/fake/template.img"),
            |_src, dst| {
                cloned.set(true);
                std::fs::write(dst, b"cloned").map_err(Into::into)
            },
            |_| panic!("fallback must not run when the clone succeeds"),
        )
        .unwrap();
        assert!(cloned.get(), "clone was invoked");
        assert_eq!(std::fs::read(&path).unwrap(), b"cloned");
        assert_eq!(
            path,
            root.path().join("runs").join("aa08").join("upper.img")
        );
    }

    #[test]
    fn template_clone_failure_falls_back_to_the_direct_writer() {
        init_tracing_capture();
        let root = tempfile::TempDir::new().unwrap();
        let path = provision_image_from_template(
            root.path(),
            "aa09",
            std::path::Path::new("/fake/template.img"),
            |_, _| Err(std::io::Error::other("not on APFS")),
            |p| std::fs::write(p, b"direct").map_err(Into::into),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"direct");
    }

    #[test]
    fn template_clone_failure_propagates_fallback_errors() {
        init_tracing_capture();
        let root = tempfile::TempDir::new().unwrap();
        let err = provision_image_from_template(
            root.path(),
            "aa0a",
            std::path::Path::new("/fake/template.img"),
            |_, _| Err(std::io::Error::other("not on APFS")),
            |_| Err(anyhow::anyhow!("writer boom")),
        )
        .unwrap_err();
        assert!(err.to_string().contains("writer boom"));
    }

    #[test]
    fn template_provision_errors_when_run_dir_cannot_be_created() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let err = provision_image_from_template(
            file.path(),
            "aa0b",
            std::path::Path::new("/fake/template.img"),
            |_, _| Ok(()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(
            err.downcast_ref::<std::io::Error>().is_some(),
            "io error from create_dir_all: {err}"
        );
    }

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
