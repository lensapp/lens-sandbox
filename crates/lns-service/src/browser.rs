/// Opens `url` in the user's default browser via the platform opener (`open` on macOS, `xdg-open` elsewhere).
pub fn open(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        crate::log::warn!("could not open {url} via {opener}: {e}");
    }
}
