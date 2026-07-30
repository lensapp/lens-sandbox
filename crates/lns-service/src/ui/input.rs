use eframe::egui;

use crate::approval_flow::window;

pub fn secret_input(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, window::BORDER);
        ui.style_mut().visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_gray(96));
        ui.add(
            egui::TextEdit::singleline(value)
                .password(true)
                .hint_text(hint)
                .margin(egui::Margin::symmetric(10, 9))
                .desired_width(f32::INFINITY),
        )
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_input_renders_masked_and_editable() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(460.0, 200.0),
            )),
            ..Default::default()
        };
        let mut value = String::from("secret");
        let _ = ctx.run_ui(input, |ui| {
            secret_input(ui, &mut value, "Enter a value");
        });
        assert_eq!(value, "secret");
    }
}
