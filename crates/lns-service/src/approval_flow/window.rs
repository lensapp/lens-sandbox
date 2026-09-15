use eframe::egui::{self, Color32, Stroke};
use std::sync::{Arc, OnceLock};
static CTX: OnceLock<egui::Context> = OnceLock::new();

pub fn install_ctx(ctx: egui::Context) {
    let _ = CTX.set(ctx);
}

pub fn ctx() -> Option<egui::Context> {
    CTX.get().cloned()
}

pub fn quiet_debug_overlays(ctx: &egui::Context) {
    #[cfg(debug_assertions)]
    ctx.all_styles_mut(|style| {
        style.debug.warn_if_rect_changes_id = false;
        style.debug.show_unaligned = false;
    });
    #[cfg(not(debug_assertions))]
    let _ = ctx;
}

pub const BG_PRIMARY: Color32 = Color32::from_rgb(0x0f, 0x10, 0x12);
pub const BG_SECONDARY: Color32 = Color32::from_rgb(0x16, 0x17, 0x19);
pub const BG_TERTIARY: Color32 = Color32::from_rgb(0x1c, 0x1e, 0x20);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2c, 0x2e);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xc4, 0xc6, 0xc8);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x70, 0x73, 0x76);
pub const TEXT_ACCENT: Color32 = Color32::from_rgb(0xf5, 0xf7, 0xf9);
pub const ACCENT_GREEN: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
pub const ACCENT_GREEN_HOVER: Color32 = Color32::from_rgb(0x6e, 0xe7, 0x9a);
pub const ACCENT_GREEN_PRESSED: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const STATUS_CRITICAL: Color32 = Color32::from_rgb(0xf4, 0x71, 0x74);
pub const STATUS_WARNING: Color32 = Color32::from_rgb(0xff, 0xb1, 0x4a);
pub const TEXT_WARN: Color32 = STATUS_WARNING;
pub const CATEGORY: Color32 = Color32::from_rgb(0x3d, 0x90, 0xce);

pub fn lds_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = BG_SECONDARY;
    v.extreme_bg_color = BG_TERTIARY;
    v.faint_bg_color = BORDER;
    v.override_text_color = Some(TEXT_PRIMARY);
    v.hyperlink_color = ACCENT_GREEN;
    v.selection.bg_fill = Color32::from_gray(64);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT_ACCENT);

    let radius = egui::CornerRadius::same(8);

    v.widgets.noninteractive.bg_fill = BG_SECONDARY;
    v.widgets.noninteractive.weak_bg_fill = BG_SECONDARY;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    v.widgets.noninteractive.corner_radius = radius;

    v.widgets.inactive.bg_fill = ACCENT_GREEN;
    v.widgets.inactive.weak_bg_fill = ACCENT_GREEN;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, BG_PRIMARY);
    v.widgets.inactive.corner_radius = radius;

    v.widgets.hovered.bg_fill = ACCENT_GREEN_HOVER;
    v.widgets.hovered.weak_bg_fill = ACCENT_GREEN_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_GREEN_HOVER);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, BG_PRIMARY);
    v.widgets.hovered.corner_radius = radius;

    v.widgets.active.bg_fill = ACCENT_GREEN_PRESSED;
    v.widgets.active.weak_bg_fill = ACCENT_GREEN_PRESSED;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_GREEN_PRESSED);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, BG_PRIMARY);
    v.widgets.active.corner_radius = radius;
    v
}

#[derive(Default)]
struct HostFonts {
    ui: Option<Vec<u8>>,
    mono: Option<Vec<u8>>,
}

/// Registers the host's system UI and monospace fonts ahead of egui's bundled set, so the approval window renders in the platform's native typeface (San Francisco on macOS) with the bundled font kept as the glyph fallback.
pub fn install_system_fonts(ctx: &egui::Context) {
    apply_host_fonts(ctx, read_host_fonts());
}

const ICON_Y_OFFSET: f32 = -2.0;

pub fn install_icon_font(ctx: &egui::Context) {
    let mut insert = egui_material_icons::font_insert();
    insert.data.tweak.y_offset = ICON_Y_OFFSET;
    ctx.add_font(insert);
}

fn apply_host_fonts(ctx: &egui::Context, host: HostFonts) {
    if let Some(defs) = build_font_defs(host) {
        ctx.set_fonts(defs);
    }
}

fn build_font_defs(host: HostFonts) -> Option<egui::FontDefinitions> {
    if host.ui.is_none() && host.mono.is_none() {
        return None;
    }
    let mut defs = egui::FontDefinitions::default();
    if let Some(bytes) = host.ui {
        prepend_font(
            &mut defs,
            "system-ui",
            bytes,
            egui::FontFamily::Proportional,
        );
    }
    if let Some(bytes) = host.mono {
        prepend_font(&mut defs, "system-mono", bytes, egui::FontFamily::Monospace);
    }
    Some(defs)
}

fn prepend_font(
    defs: &mut egui::FontDefinitions,
    name: &str,
    bytes: Vec<u8>,
    family: egui::FontFamily,
) {
    defs.font_data
        .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    defs.families
        .entry(family)
        .or_default()
        .insert(0, name.to_owned());
}

fn read_host_fonts() -> HostFonts {
    use crate::approval_flow::system_font;
    HostFonts {
        ui: system_font::ui_font_bytes(),
        mono: system_font::mono_font_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::inbox::{self, ApprovalInbox};

    #[test]
    fn install_publishes_state_and_ctx_and_getters_return_them() {
        let s = ApprovalInbox::new();
        inbox::install(s.clone());
        install_ctx(egui::Context::default());
        let got_state = inbox::get().expect("global state should be installed");
        assert!(Arc::ptr_eq(&s, &got_state) || Arc::strong_count(&got_state) >= 2);
        assert!(ctx().is_some(), "global ctx should be installed");
    }

    #[test]
    fn lds_visuals_uses_dark_palette_and_green_accent() {
        let v = lds_visuals();
        assert_eq!(v.panel_fill, Color32::TRANSPARENT);
        assert_eq!(v.window_fill, BG_SECONDARY);
        assert_eq!(v.override_text_color, Some(TEXT_PRIMARY));
        assert_eq!(v.selection.bg_fill, Color32::from_gray(64));
        assert_eq!(v.hyperlink_color, ACCENT_GREEN);
        assert!(v.dark_mode);
    }

    #[test]
    fn quiet_debug_overlays_turns_off_the_red_and_orange_debug_paint() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| {
            s.debug.warn_if_rect_changes_id = true;
            s.debug.show_unaligned = true;
        });
        quiet_debug_overlays(&ctx);
        let debug = ctx.global_style().debug;
        assert!(!debug.warn_if_rect_changes_id);
        assert!(!debug.show_unaligned);
    }

    #[test]
    fn build_font_defs_is_none_when_no_host_font_is_available() {
        assert!(build_font_defs(HostFonts::default()).is_none());
    }

    #[test]
    fn build_font_defs_puts_the_system_ui_font_ahead_of_the_bundled_fallback() {
        let defs = build_font_defs(HostFonts {
            ui: Some(b"ui-bytes".to_vec()),
            mono: None,
        })
        .expect("a ui font yields definitions");
        let proportional = &defs.families[&egui::FontFamily::Proportional];
        assert_eq!(
            proportional.first().map(String::as_str),
            Some("system-ui"),
            "the system font is tried first so text renders in the native typeface"
        );
        assert!(
            proportional.len() > 1,
            "egui's bundled font stays as the glyph fallback"
        );
        assert!(
            !defs.families[&egui::FontFamily::Monospace].contains(&"system-mono".to_string()),
            "no monospace font was supplied, so the bundled mono is left untouched"
        );
    }

    #[test]
    fn build_font_defs_registers_the_monospace_font_ahead_of_the_fallback() {
        let defs = build_font_defs(HostFonts {
            ui: None,
            mono: Some(b"mono-bytes".to_vec()),
        })
        .expect("a mono font yields definitions");
        assert_eq!(
            defs.families[&egui::FontFamily::Monospace]
                .first()
                .map(String::as_str),
            Some("system-mono")
        );
    }

    #[test]
    fn apply_host_fonts_installs_a_supplied_font_without_panicking() {
        let ctx = egui::Context::default();
        apply_host_fonts(
            &ctx,
            HostFonts {
                ui: Some(b"ui-bytes".to_vec()),
                mono: None,
            },
        );
    }

    #[test]
    fn apply_host_fonts_leaves_the_defaults_when_no_host_font_is_available() {
        let ctx = egui::Context::default();
        apply_host_fonts(&ctx, HostFonts::default());
    }

    #[test]
    fn read_host_fonts_returns_without_panicking_on_this_host() {
        let _ = read_host_fonts();
    }

    #[test]
    fn install_system_fonts_applies_host_fonts_without_panicking() {
        install_system_fonts(&egui::Context::default());
    }

    #[test]
    fn install_icon_font_lifts_the_glyph_baseline() {
        install_icon_font(&egui::Context::default());
        assert_eq!(ICON_Y_OFFSET, -2.0);
    }
}
