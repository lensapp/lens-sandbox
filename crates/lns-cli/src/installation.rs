use std::ffi::OsStr;
use std::path::Path;

pub fn require_loose_binaries(executable: &Path) -> anyhow::Result<()> {
    let bundle = executable
        .ancestors()
        .filter(|path| path.file_name() == Some(OsStr::new("Contents")))
        .filter_map(Path::parent)
        .find(|path| path.extension() == Some(OsStr::new("app")));
    if let Some(bundle) = bundle {
        anyhow::bail!(
            "this CLI belongs to {}; replace or remove the complete app instead. \
             Updating or uninstalling individual helpers would invalidate its signature.",
            bundle.display()
        );
    }
    Ok(())
}
