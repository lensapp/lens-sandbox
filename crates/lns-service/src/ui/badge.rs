use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

use crate::approval_flow::window;
use crate::ui::theme;

fn badge_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 16)
}

fn badge(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(badge_fill())
        .stroke(Stroke::new(1.0_f32, window::BORDER))
        .corner_radius(CornerRadius::same(theme::BADGE_CORNER_RADIUS))
        .inner_margin(Margin::symmetric(theme::BADGE_PAD_X, theme::BADGE_PAD_Y))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(theme::FONT_BADGE)
                    .color(window::TEXT_MUTED),
            );
        });
}

/// One profile the user can pick. Badge-shaped but painted from the button palette, because a chip is a control and the badges above it are not — a solid surface and brighter text where a badge is translucent and muted, and the card's action colour when it is the chosen one.
pub fn chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (fill, stroke, text) = if selected {
        (
            super::button::tint(window::CATEGORY, 40),
            super::button::tint(window::CATEGORY, 140),
            window::CATEGORY,
        )
    } else {
        (window::BG_TERTIARY, window::BORDER, window::TEXT_PRIMARY)
    };
    // The frame's own response, not the label's: the padding is on the frame, so a click on a chip's edge must count.
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, stroke))
        .corner_radius(CornerRadius::same(theme::BADGE_CORNER_RADIUS))
        .inner_margin(Margin::symmetric(theme::CHIP_PAD_X, theme::CHIP_PAD_Y))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(label)
                        .size(theme::FONT_BODY)
                        .color(text),
                )
                .selectable(false),
            );
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Every chip on one wrapping line, so a card with several accounts grows down rather than off the edge.
pub fn chips<'a>(
    ui: &mut egui::Ui,
    labels: impl IntoIterator<Item = (&'a str, bool)>,
) -> Option<String> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = theme::BADGE_GAP;
        for (label, selected) in labels {
            if chip(ui, label, selected).clicked() {
                picked = Some(label.to_string());
            }
        }
    });
    picked
}

pub fn badges<I, S>(ui: &mut egui::Ui, labels: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::BADGE_GAP;
        for label in labels {
            badge(ui, label.as_ref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_frame(mut build: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(460.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| build(ui));
    }

    #[test]
    fn badges_render_a_chip_per_label() {
        run_frame(|ui| badges(ui, ["TCP", "port 443"]));
    }

    #[test]
    fn badges_handle_an_empty_list() {
        run_frame(|ui| badges(ui, Vec::<&str>::new()));
    }
}
