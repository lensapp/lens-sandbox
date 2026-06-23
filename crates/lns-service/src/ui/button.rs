use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Stroke, Vec2};

use crate::approval_flow::window;
use crate::ui::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

pub struct Button<'a> {
    label: &'a str,
    kind: ButtonKind,
    enabled: bool,
    min_size: Vec2,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str, kind: ButtonKind) -> Self {
        Self {
            label,
            kind,
            enabled: true,
            min_size: Vec2::new(0.0, theme::BUTTON_MIN_HEIGHT),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = Vec2::new(min_size.x, min_size.y.max(theme::BUTTON_MIN_HEIGHT));
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> egui::Response {
        ui.add_enabled_ui(self.enabled, |ui| {
            style_button(ui.style_mut(), self.kind);
            let text = RichText::new(self.label).font(FontId::new(
                theme::BUTTON_FONT_SIZE,
                egui::FontFamily::Proportional,
            ));
            ui.add(egui::Button::new(text).truncate().min_size(self.min_size))
        })
        .inner
    }
}

struct StateColors {
    fill: Color32,
    stroke: Color32,
    fg: Color32,
}

fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

fn palette(kind: ButtonKind) -> [StateColors; 3] {
    match kind {
        ButtonKind::Primary => [
            StateColors {
                fill: tint(window::CATEGORY, 40),
                stroke: tint(window::CATEGORY, 140),
                fg: window::CATEGORY,
            },
            StateColors {
                fill: tint(window::CATEGORY, 74),
                stroke: window::CATEGORY,
                fg: window::CATEGORY,
            },
            StateColors {
                fill: tint(window::CATEGORY, 110),
                stroke: window::CATEGORY,
                fg: window::CATEGORY,
            },
        ],
        ButtonKind::Secondary => [
            StateColors {
                fill: window::BG_TERTIARY,
                stroke: window::BORDER,
                fg: window::TEXT_PRIMARY,
            },
            StateColors {
                fill: window::BORDER,
                stroke: window::BORDER,
                fg: window::TEXT_ACCENT,
            },
            StateColors {
                fill: window::BG_TERTIARY,
                stroke: window::BORDER,
                fg: window::TEXT_ACCENT,
            },
        ],
        ButtonKind::Danger => [
            StateColors {
                fill: tint(window::STATUS_CRITICAL, 40),
                stroke: tint(window::STATUS_CRITICAL, 90),
                fg: window::STATUS_CRITICAL,
            },
            StateColors {
                fill: tint(window::STATUS_CRITICAL, 74),
                stroke: window::STATUS_CRITICAL,
                fg: window::STATUS_CRITICAL,
            },
            StateColors {
                fill: tint(window::STATUS_CRITICAL, 110),
                stroke: window::STATUS_CRITICAL,
                fg: window::STATUS_CRITICAL,
            },
        ],
    }
}

fn style_button(style: &mut egui::Style, kind: ButtonKind) {
    let [inactive, hovered, active] = palette(kind);
    let radius = CornerRadius::same(theme::BUTTON_CORNER_RADIUS);
    set_state(&mut style.visuals.widgets.inactive, &inactive, radius);
    set_state(&mut style.visuals.widgets.hovered, &hovered, radius);
    set_state(&mut style.visuals.widgets.active, &active, radius);
    style.spacing.button_padding = theme::BUTTON_PADDING;
}

fn set_state(w: &mut egui::style::WidgetVisuals, c: &StateColors, radius: CornerRadius) {
    w.bg_fill = c.fill;
    w.weak_bg_fill = c.fill;
    w.bg_stroke = Stroke::new(1.0, c.stroke);
    w.fg_stroke = Stroke::new(1.0, c.fg);
    w.corner_radius = radius;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_is_a_tonal_blue_not_a_solid_fill() {
        let [inactive, _, _] = palette(ButtonKind::Primary);
        assert!(
            inactive.fill.a() < 255,
            "primary fill is a translucent tint, not a solid block"
        );
        assert_eq!(inactive.fg, window::CATEGORY, "blue label on the tint");
    }

    #[test]
    fn secondary_is_a_neutral_button_not_a_red_one() {
        let [inactive, _, _] = palette(ButtonKind::Secondary);
        assert_eq!(inactive.fill, window::BG_TERTIARY);
        assert_eq!(inactive.stroke, window::BORDER);
        assert_ne!(
            inactive.stroke,
            window::STATUS_CRITICAL,
            "a neutral dismissal must not read as a destructive action"
        );
    }

    #[test]
    fn danger_is_a_tonal_red_matching_the_primary_treatment() {
        let [inactive, hovered, _] = palette(ButtonKind::Danger);
        assert!(
            inactive.fill.a() < 255 && hovered.fill.a() < 255,
            "danger uses a translucent tint that intensifies on hover, not a solid fill"
        );
        assert_eq!(inactive.fg, window::STATUS_CRITICAL, "red label throughout");
    }

    #[test]
    fn style_button_applies_the_macos_corner_radius_and_padding() {
        let mut style = egui::Style::default();
        style_button(&mut style, ButtonKind::Primary);
        assert_eq!(
            style.visuals.widgets.inactive.corner_radius,
            CornerRadius::same(theme::BUTTON_CORNER_RADIUS)
        );
        assert_eq!(style.spacing.button_padding, theme::BUTTON_PADDING);
    }

    #[test]
    fn min_size_never_drops_below_the_button_floor_height() {
        let b = Button::new("x", ButtonKind::Primary).min_size(Vec2::new(120.0, 0.0));
        assert_eq!(b.min_size.y, theme::BUTTON_MIN_HEIGHT);
        assert_eq!(b.min_size.x, 120.0);
    }

    #[test]
    fn show_renders_every_kind_enabled_and_disabled() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(460.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            for kind in [
                ButtonKind::Primary,
                ButtonKind::Secondary,
                ButtonKind::Danger,
            ] {
                let _ = Button::new("label", kind).show(ui);
            }
            let _ = Button::new("off", ButtonKind::Primary)
                .enabled(false)
                .min_size(Vec2::new(120.0, 0.0))
                .show(ui);
        });
    }
}
