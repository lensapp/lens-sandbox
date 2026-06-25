#[cfg(target_os = "macos")]
pub fn ui_font_bytes() -> Option<Vec<u8>> {
    resolve(objc2_core_text::CTFontUIFontType::System)
}

#[cfg(target_os = "macos")]
pub fn mono_font_bytes() -> Option<Vec<u8>> {
    resolve(objc2_core_text::CTFontUIFontType::UserFixedPitch)
}

#[cfg(target_os = "macos")]
fn resolve(ui_type: objc2_core_text::CTFontUIFontType) -> Option<Vec<u8>> {
    use objc2_core_foundation::{CFURL, CFURLPathStyle};
    use objc2_core_text::{CTFont, kCTFontURLAttribute};

    // SAFETY: CTFontCreateUIFontForLanguage takes a UI-font-type constant with default size/language and returns an owned font or null.
    let font = unsafe { CTFont::new_ui_font_for_language(ui_type, 0.0, None) }?;
    // SAFETY: reads the framework's URL-attribute key and copies that attribute off a live CTFont, yielding an owned value or null.
    let url_attr = unsafe { font.attribute(kCTFontURLAttribute) }?;
    let url = url_attr.downcast_ref::<CFURL>()?;
    let path = url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)?;
    std::fs::read(path.to_string()).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn ui_font_bytes() -> Option<Vec<u8>> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn mono_font_bytes() -> Option<Vec<u8>> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn core_text_resolves_a_real_system_ui_font_on_this_host() {
        let bytes = ui_font_bytes().expect("macOS resolves a system UI font via Core Text");
        assert!(
            bytes.len() > 4096,
            "a real font file is more than a few bytes"
        );
    }

    #[test]
    fn core_text_resolves_a_real_monospace_font_on_this_host() {
        let bytes =
            mono_font_bytes().expect("macOS resolves a fixed-pitch system font via Core Text");
        assert!(
            bytes.len() > 4096,
            "a real font file is more than a few bytes"
        );
    }
}
