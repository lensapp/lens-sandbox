use std::path::Path;

pub fn require_loose_binaries(executable: &Path) -> anyhow::Result<()> {
    let bundle = lns_ipc::desktop_bundle(executable);
    if let Some(bundle) = bundle {
        anyhow::bail!(
            "this CLI belongs to {}; replace or remove the complete app instead. \
             Updating or uninstalling individual helpers would invalidate its signature.",
            bundle.display()
        );
    }
    Ok(())
}
