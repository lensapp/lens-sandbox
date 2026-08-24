use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};
use egui_material_icons::MaterialIcon;

use crate::approval_flow::window;
use crate::ui::theme;

fn card_fill() -> Color32 {
    let c = window::BG_SECONDARY;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), theme::CARD_FILL_ALPHA)
}

fn footer_fill() -> Color32 {
    let c = window::BG_PRIMARY;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), theme::CARD_FILL_ALPHA)
}

pub fn eyebrow(ui: &mut egui::Ui, icon: MaterialIcon, label: &str) {
    let format = |size: f32, family: egui::FontFamily| egui::TextFormat {
        font_id: egui::FontId::new(size, family),
        color: window::CATEGORY,
        valign: egui::Align::Center,
        line_height: Some(theme::FONT_EYEBROW_ICON),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        icon.codepoint,
        0.0,
        format(theme::FONT_EYEBROW_ICON, icon.font_family()),
    );
    job.append(
        label,
        6.0,
        format(theme::FONT_EYEBROW, egui::FontFamily::Proportional),
    );
    ui.label(job);
}

fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(card_fill())
        .stroke(Stroke::new(1.0_f32, window::BORDER))
        .corner_radius(CornerRadius::same(theme::CARD_CORNER_RADIUS))
}

pub fn card<R>(
    ui: &mut egui::Ui,
    width: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner_width = (width - 2.0 * theme::CARD_PADDING as f32).max(0.0);
    card_frame().show(ui, |ui| {
        ui.set_width(width);
        egui::Frame::new()
            .inner_margin(Margin::same(theme::CARD_PADDING))
            .show(ui, |ui| {
                ui.set_width(inner_width);
                add_contents(ui)
            })
            .inner
    })
}

pub fn card_sectioned<R>(
    ui: &mut egui::Ui,
    width: f32,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner_width = (width - 2.0 * theme::CARD_PADDING as f32).max(0.0);
    let footer_corners = CornerRadius {
        nw: 0,
        ne: 0,
        sw: theme::CARD_CORNER_RADIUS,
        se: theme::CARD_CORNER_RADIUS,
    };
    card_frame().show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::Frame::new()
            .inner_margin(Margin::same(theme::CARD_PADDING))
            .show(ui, |ui| {
                ui.set_width(inner_width);
                body(ui);
            });
        egui::Frame::new()
            .fill(footer_fill())
            .corner_radius(footer_corners)
            .inner_margin(Margin::same(theme::CARD_PADDING))
            .show(ui, |ui| {
                ui.set_width(inner_width);
                footer(ui)
            })
            .inner
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_frame(mut build: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        window::install_icon_font(&ctx);
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
            let out = card(ui, 344.0, |ui| {
                ui.label("body");
                42
            });
            assert_eq!(out.inner, 42);
        });
    }

    #[test]
    fn eyebrow_renders_without_panicking() {
        run_frame(|ui| eyebrow(ui, egui_material_icons::icons::ICON_PUBLIC, "NETWORK"));
    }

    #[test]
    fn card_sectioned_renders_body_and_footer_and_returns_the_footer_value() {
        run_frame(|ui| {
            let out = card_sectioned(
                ui,
                344.0,
                |ui| {
                    ui.label("body");
                },
                |ui| {
                    ui.label("footer");
                    7
                },
            );
            assert_eq!(out.inner, 7);
        });
    }
}
