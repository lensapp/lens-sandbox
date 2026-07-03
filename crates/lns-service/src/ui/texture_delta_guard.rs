use eframe::egui;
use egui::epaint::textures::TexturesDelta;

/// Recovers the texture updates eframe discards for passes of unpainted windows (emilk/egui: `run_ui_and_paint` skips `paint_and_update_textures` when the window is hidden, losing the pass's drained `TexturesDelta` and desyncing the GPU font atlas until restart) by stashing them and replaying them ahead of the next painted pass.
#[derive(Default)]
pub struct TextureDeltaGuard {
    pass_painted: Vec<bool>,
    stashed: TexturesDelta,
}

pub fn install(ctx: &egui::Context) {
    ctx.add_plugin(TextureDeltaGuard::default());
}

impl egui::Plugin for TextureDeltaGuard {
    fn debug_name(&self) -> &'static str {
        "lns_texture_delta_guard"
    }

    fn on_begin_pass(&mut self, ui: &mut egui::Ui) {
        let painted = ui.ctx().input(|i| i.viewport().visible().unwrap_or(true));
        self.pass_painted.push(painted);
    }

    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        let painted = self.pass_painted.pop().unwrap_or(true);
        if !painted {
            self.stashed
                .append(std::mem::take(&mut output.textures_delta));
        } else if !self.stashed.is_empty() {
            let mut deltas = std::mem::take(&mut self.stashed);
            deltas.append(std::mem::take(&mut output.textures_delta));
            output.textures_delta = deltas;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_pass(
        ctx: &egui::Context,
        minimized_and_occluded: Option<bool>,
        mut in_pass: impl FnMut(&egui::Context),
    ) -> egui::FullOutput {
        let mut info = egui::ViewportInfo::default();
        if let Some(hidden) = minimized_and_occluded {
            info.minimized = Some(hidden);
            info.occluded = Some(hidden);
        }
        let mut input = egui::RawInput::default();
        input.viewports.insert(egui::ViewportId::ROOT, info);
        ctx.run_ui(input, |ui| in_pass(ui.ctx()))
    }

    fn alloc_texture(ctx: &egui::Context, name: &str) -> egui::TextureId {
        ctx.tex_manager().write().alloc(
            name.to_owned(),
            egui::ColorImage::example().into(),
            egui::TextureOptions::LINEAR,
        )
    }

    fn set_position(output: &egui::FullOutput, id: egui::TextureId) -> usize {
        output
            .textures_delta
            .set
            .iter()
            .position(|(set_id, _)| *set_id == id)
            .expect("texture id missing from output deltas")
    }

    #[test]
    fn an_unpainted_pass_hands_the_backend_no_texture_deltas() {
        let ctx = egui::Context::default();
        install(&ctx);
        let output = run_pass(&ctx, Some(true), |ctx| {
            alloc_texture(ctx, "hidden-pass");
        });
        assert!(output.textures_delta.is_empty());
    }

    #[test]
    fn the_next_painted_pass_replays_stashed_deltas_before_its_own() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut hidden_id = None;
        run_pass(&ctx, Some(true), |ctx| {
            hidden_id = Some(alloc_texture(ctx, "stashed"));
        });
        let mut painted_id = None;
        let output = run_pass(&ctx, Some(false), |ctx| {
            painted_id = Some(alloc_texture(ctx, "current"));
        });
        let stashed_pos = set_position(&output, hidden_id.unwrap());
        let current_pos = set_position(&output, painted_id.unwrap());
        assert!(stashed_pos < current_pos);
        assert!(
            ctx.with_plugin(|guard: &mut TextureDeltaGuard| guard.stashed.is_empty())
                .unwrap()
        );
    }

    #[test]
    fn frees_from_an_unpainted_pass_reach_the_next_painted_pass() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut id = None;
        run_pass(&ctx, Some(false), |ctx| {
            id = Some(alloc_texture(ctx, "victim"));
        });
        run_pass(&ctx, Some(true), |ctx| {
            ctx.tex_manager().write().free(id.unwrap());
        });
        let output = run_pass(&ctx, Some(false), |_| {});
        assert!(output.textures_delta.free.contains(&id.unwrap()));
    }

    #[test]
    fn a_painted_pass_with_nothing_stashed_keeps_its_deltas() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut id = None;
        let output = run_pass(&ctx, Some(false), |ctx| {
            id = Some(alloc_texture(ctx, "plain"));
        });
        set_position(&output, id.unwrap());
    }

    #[test]
    fn unknown_visibility_counts_as_painted() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut id = None;
        let output = run_pass(&ctx, None, |ctx| {
            id = Some(alloc_texture(ctx, "unknown"));
        });
        set_position(&output, id.unwrap());
    }

    #[test]
    fn an_output_without_a_matching_pass_counts_as_painted() {
        let mut guard = TextureDeltaGuard::default();
        let mut output = egui::FullOutput::default();
        output.textures_delta.free.push(egui::TextureId::Managed(7));
        egui::Plugin::output_hook(&mut guard, &mut output);
        assert!(!output.textures_delta.is_empty());
        assert!(guard.stashed.is_empty());
    }

    #[test]
    fn the_debug_name_identifies_the_guard() {
        assert_eq!(
            egui::Plugin::debug_name(&TextureDeltaGuard::default()),
            "lns_texture_delta_guard"
        );
    }
}
