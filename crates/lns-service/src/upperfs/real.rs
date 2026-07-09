use std::time::{SystemTime, UNIX_EPOCH};

use super::write_ext4;

fn mkfs_time_now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn fresh_plan() -> super::Plan {
    super::Plan::new(
        super::DEFAULT_SIZE_BYTES,
        rand::random(),
        "lns-upper",
        mkfs_time_now(),
    )
}

#[cfg(target_os = "macos")]
static TEMPLATE: tokio::sync::OnceCell<std::path::PathBuf> = tokio::sync::OnceCell::const_new();

// The template carries one random uuid per service process; every clone shares it, which is harmless because each run's guest kernel only ever mounts its own upper disk.
#[cfg(target_os = "macos")]
async fn ensure_template(root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let path = TEMPLATE
        .get_or_try_init(|| async {
            let path = root.join("upper-template.img");
            let write_target = path.clone();
            tokio::task::spawn_blocking(move || write_ext4(&fresh_plan(), &write_target))
                .await??;
            Ok::<_, anyhow::Error>(path)
        })
        .await?;
    Ok(path.clone())
}

#[cfg(target_os = "macos")]
fn clonefile(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let src = std::ffi::CString::new(src.as_os_str().as_bytes())?;
    let dst = std::ffi::CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: both pointers reference NUL-terminated buffers that outlive the call.
    let rc = unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
pub async fn provision(run_id: &str) -> anyhow::Result<std::path::PathBuf> {
    let root = crate::cache::root()?;
    let template = ensure_template(&root).await?;
    let run_id = run_id.to_string();
    tokio::task::spawn_blocking(move || {
        super::provision_image_from_template(&root, &run_id, &template, clonefile, |path| {
            write_ext4(&fresh_plan(), path)
        })
    })
    .await?
}

#[cfg(not(target_os = "macos"))]
pub async fn provision(run_id: &str) -> anyhow::Result<std::path::PathBuf> {
    let root = crate::cache::root()?;
    let run_id = run_id.to_string();
    tokio::task::spawn_blocking(move || {
        super::provision_image(&root, &run_id, rand::random(), mkfs_time_now(), write_ext4)
    })
    .await?
}
