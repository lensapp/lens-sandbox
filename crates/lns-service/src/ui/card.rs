use eframe::egui::{self, CornerRadius, Margin, Stroke};

use crate::approval_flow::window;
use crate::ui::theme;

/// A standalone macOS-notification-style panel: rounded fill and a hairline border, separated from its neighbours by the stack's gaps on the transparent window.
pub fn card<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner_width = (ui.available_width() - 2.0 * theme::CARD_PADDING as f32).max(0.0);
    egui::Frame::new()
        .fill(window::BG_SECONDARY)
        .stroke(Stroke::new(1.0, window::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_CORNER_RADIUS))
        .inner_margin(Margin::same(theme::CARD_PADDING))
        .show(ui, |ui| {
            ui.set_min_width(inner_width);
            add_contents(ui)
        })
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
    fn card_wraps_its_contents_and_returns_the_inner_value() {
        run_frame(|ui| {
            let out = card(ui, |ui| {
                ui.label("body");
                42
            });
            assert_eq!(out.inner, 42);
        });
    }
}
