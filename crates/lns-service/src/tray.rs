use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context;
use eframe::egui;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::PendingPrompt;
use crate::approval_flow::window::{self, CredentialCardPrompt, Snapshot, WindowState};
use crate::credential_flow::session::CredentialDecisionRequest;
use crate::credential_flow::store::CredentialEntry;
use crate::shutdown::Shutdown;

const WINDOW_WIDTH: f32 = 460.0;
const WINDOW_HEIGHT: f32 = 300.0;
const SCREEN_EDGE_MARGIN: f32 = 20.0;

pub fn run_tray(
    shutdown: Arc<Shutdown>,
    ipc_handle: JoinHandle<anyhow::Result<()>>,
    window_state: Arc<WindowState>,
) -> anyhow::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Lens Sandbox")
        .with_position([0.0, 0.0])
        .with_visible(false)
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_mouse_passthrough(true)
        .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT]);

    let mut native_options = eframe::NativeOptions {
        viewport,
        run_and_return: true,
        ..Default::default()
    };
    install_activation_policy(&mut native_options);

    let app_shutdown = shutdown.clone();
    let result = eframe::run_native(
        "lns-service",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(window::lds_visuals());
            window::install_ctx(cc.egui_ctx.clone());
            let app = TrayApp::new(cc.egui_ctx.clone(), app_shutdown, window_state)
                .map_err(|e| format!("{e:#}"))?;
            Ok(Box::new(app))
        }),
    );

    shutdown.signal();
    let _ = ipc_handle.join();

    result.map_err(|e| anyhow::anyhow!("eframe run_native failed: {e}"))
}

struct TrayApp {
    shutdown: Arc<Shutdown>,
    window_state: Arc<WindowState>,
    _tray: tray_icon::TrayIcon,
    last_visible: bool,
    positioned: bool,
    /// Kept on `TrayApp` (not [`WindowState`]) so the snapshot passed to [`render_card`] stays immutable.
    credential_inputs: HashMap<String, String>,
}

impl TrayApp {
    fn new(
        ctx: egui::Context,
        shutdown: Arc<Shutdown>,
        window_state: Arc<WindowState>,
    ) -> anyhow::Result<Self> {
        let menu = Menu::new();
        let quit_item = MenuItem::new(
            "Quit Lens Sandbox",
            true,
            Some(Accelerator::new(Some(Modifiers::META), Code::KeyQ)),
        );
        menu.append(&quit_item)
            .context("failed to append quit menu item")?;
        let quit_id = quit_item.id().clone();

        let icon = load_icon().context("load embedded tray icon")?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Lens Sandbox")
            .with_icon(icon)
            .with_icon_as_template(true)
            .build()
            .context("build tray icon")?;

        let menu_ctx = ctx.clone();
        let menu_shutdown = shutdown.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == quit_id {
                menu_shutdown.signal();
            }
            menu_ctx.request_repaint();
        }));

        let watch_shutdown = shutdown.clone();
        let watch_ctx = ctx;
        std::thread::spawn(move || {
            watch_shutdown.wait_sync();
            watch_ctx.request_repaint();
        });

        Ok(Self {
            shutdown,
            window_state,
            _tray: tray,
            last_visible: false,
            positioned: false,
            credential_inputs: HashMap::new(),
        })
    }

    fn sync_viewport_visibility(&mut self, ctx: &egui::Context) {
        if !self.positioned
            && let Some(pos) = top_right_position(ctx)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            self.positioned = true;
        }

        let should_show = self.window_state.pending_count() > 0
            || !self.window_state.snapshot().informs.is_empty();
        match visibility_transition(should_show, self.last_visible) {
            VisibilityTransition::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.last_visible = true;
            }
            VisibilityTransition::Hide => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(true));
                self.last_visible = false;
            }
            VisibilityTransition::Unchanged => {}
        }
    }
}

impl eframe::App for TrayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shutdown.is_set() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.sync_viewport_visibility(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snapshot = self.window_state.snapshot();
        prune_credential_inputs(&mut self.credential_inputs, &snapshot);

        match render_card(ui, &snapshot, &mut self.credential_inputs) {
            Some(CardAction::Decide { id, decision }) => {
                self.window_state.decide(&id, decision);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DecideCredential { id, request }) => {
                self.credential_inputs.remove(&id);
                self.window_state.decide_credential(&id, request);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DismissInform) => {
                self.window_state.dismiss_first_inform();
                ui.ctx().request_repaint();
            }
            None => {}
        }
    }
}

const BTN_WIDTH: f32 = 188.0;
const BTN_HEIGHT: f32 = 38.0;
const BTN_GAP: f32 = 12.0;

#[derive(Debug, PartialEq, Eq)]
enum CardAction {
    Decide {
        id: String,
        decision: Decision,
    },
    DecideCredential {
        id: String,
        request: CredentialDecisionRequest,
    },
    DismissInform,
}

fn render_card(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
) -> Option<CardAction> {
    use egui::{CornerRadius, Frame, Margin, Stroke};

    if snapshot.pending.is_empty()
        && snapshot.pending_credentials.is_empty()
        && snapshot.informs.is_empty()
    {
        return None;
    }

    Frame::new()
        .fill(window::BG_SECONDARY)
        .stroke(Stroke::new(1.0, window::BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(22))
        .outer_margin(Margin::same(8))
        .show(ui, |ui| {
            if let Some(action) = render_inform_banner(ui, snapshot) {
                return Some(action);
            }
            // Network cards take precedence over credential cards when both are pending (S8).
            if let Some(prompt) = snapshot.pending.first() {
                return render_network_card(ui, prompt, snapshot.pending.len());
            }
            if let Some(prompt) = snapshot.pending_credentials.first() {
                let input = credential_inputs.entry(prompt.id.clone()).or_default();
                return render_credential_card(
                    ui,
                    prompt,
                    input,
                    snapshot.pending_credentials.len(),
                );
            }
            None
        })
        .inner
}

fn render_inform_banner(ui: &mut egui::Ui, snapshot: &Snapshot) -> Option<CardAction> {
    use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Sense, Stroke};

    let msg = snapshot.informs.first()?;
    let banner = Frame::new()
        .fill(window::BG_TERTIARY)
        .stroke(Stroke::new(1.0, window::STATUS_WARNING))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(window::STATUS_WARNING, "⚠");
                ui.colored_label(window::TEXT_PRIMARY, RichText::new(msg).size(12.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.colored_label(window::TEXT_MUTED, RichText::new("✕  dismiss").size(10.5));
                });
            });
        });
    ui.add_space(14.0);
    if banner.response.interact(Sense::click()).clicked() {
        Some(CardAction::DismissInform)
    } else {
        None
    }
}

fn render_network_card(
    ui: &mut egui::Ui,
    prompt: &PendingPrompt,
    pending_count: usize,
) -> Option<CardAction> {
    use egui::RichText;

    let id = prompt.id.clone();

    render_card_header(ui, "APPROVAL NEEDED", pending_count);

    ui.add_space(6.0);
    ui.label(
        RichText::new(&prompt.host)
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(&prompt.action)
            .size(12.0)
            .monospace()
            .color(window::TEXT_MUTED),
    );

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut chosen: Option<Decision> = None;
    ui.horizontal(|ui| {
        if primary_button(ui, "Allow once").clicked() {
            chosen = Some(Decision::AllowOnce);
        }
        ui.add_space(BTN_GAP);
        if primary_button(ui, "Allow always").clicked() {
            chosen = Some(Decision::AllowAlways);
        }
    });
    ui.add_space(BTN_GAP);
    ui.horizontal(|ui| {
        if deny_button(ui, "Deny once").clicked() {
            chosen = Some(Decision::DenyOnce);
        }
        ui.add_space(BTN_GAP);
        if deny_button(ui, "Deny always").clicked() {
            chosen = Some(Decision::DenyAlways);
        }
    });

    chosen.map(|decision| CardAction::Decide { id, decision })
}

fn render_credential_card(
    ui: &mut egui::Ui,
    prompt: &CredentialCardPrompt,
    input: &mut String,
    pending_count: usize,
) -> Option<CardAction> {
    use egui::RichText;

    let id = prompt.id.clone();

    render_card_header(ui, "CREDENTIAL NEEDED", pending_count);

    ui.add_space(6.0);
    ui.label(
        RichText::new(&prompt.credential_id)
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(&prompt.action)
            .size(12.0)
            .monospace()
            .color(window::TEXT_MUTED),
    );

    if !prompt.host_value_available {
        ui.add_space(8.0);
        ui.colored_label(
            window::TEXT_MUTED,
            RichText::new("No credential detected on host.")
                .size(11.5)
                .italics(),
        );
    }

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut chosen: Option<CredentialDecisionRequest> = None;

    if prompt.host_value_available {
        ui.horizontal(|ui| {
            if primary_button(ui, "Use from host").clicked() {
                chosen = Some(CredentialDecisionRequest::Allow(
                    CredentialEntry::HostDetect,
                ));
            }
        });
        ui.add_space(BTN_GAP);
    }

    ui.label(
        RichText::new("Or enter a value")
            .size(11.0)
            .color(window::TEXT_MUTED),
    );
    ui.add_space(4.0);
    let text_edit = egui::TextEdit::singleline(input)
        .password(true)
        .desired_width(BTN_WIDTH * 2.0 + BTN_GAP);
    ui.add(text_edit);
    ui.add_space(BTN_GAP);
    ui.horizontal(|ui| {
        let submit_enabled = !input.trim().is_empty();
        let submit_resp = ui.add_enabled(
            submit_enabled,
            egui::Button::new(
                RichText::new("Submit value")
                    .color(window::BG_PRIMARY)
                    .strong()
                    .size(13.0),
            )
            .min_size(egui::vec2(BTN_WIDTH, BTN_HEIGHT)),
        );
        if submit_resp.clicked() && submit_enabled {
            chosen = Some(CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: input.trim().to_string(),
            }));
        }
        ui.add_space(BTN_GAP);
        if deny_button(ui, "Deny").clicked() {
            chosen = Some(CredentialDecisionRequest::Deny);
        }
    });

    chosen.map(|request| CardAction::DecideCredential { id, request })
}

fn render_card_header(ui: &mut egui::Ui, label: &str, pending_count: usize) {
    use egui::{Align, Layout, RichText};

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(10.5)
                .strong()
                .color(window::ACCENT_GREEN),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if pending_count > 1 {
                ui.label(
                    RichText::new(format!("1 of {pending_count}"))
                        .size(10.5)
                        .color(window::TEXT_MUTED),
                );
            }
        });
    });
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let text = egui::RichText::new(label)
        .color(window::BG_PRIMARY)
        .strong()
        .size(13.0);
    ui.add_sized([BTN_WIDTH, BTN_HEIGHT], egui::Button::new(text))
}

fn deny_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let text = egui::RichText::new(label).size(13.0);
    ui.scope(|ui| {
        let style = ui.style_mut();
        let critical = egui::Stroke::new(1.0, window::STATUS_CRITICAL);
        let on_critical = egui::Stroke::new(1.0, window::TEXT_ACCENT);

        style.visuals.widgets.inactive.bg_fill = window::BG_TERTIARY;
        style.visuals.widgets.inactive.weak_bg_fill = window::BG_TERTIARY;
        style.visuals.widgets.inactive.bg_stroke = critical;
        style.visuals.widgets.inactive.fg_stroke = critical;

        style.visuals.widgets.hovered.bg_fill = window::STATUS_CRITICAL;
        style.visuals.widgets.hovered.weak_bg_fill = window::STATUS_CRITICAL;
        style.visuals.widgets.hovered.bg_stroke = critical;
        style.visuals.widgets.hovered.fg_stroke = on_critical;

        style.visuals.widgets.active.bg_fill = window::STATUS_CRITICAL;
        style.visuals.widgets.active.weak_bg_fill = window::STATUS_CRITICAL;
        style.visuals.widgets.active.bg_stroke = critical;
        style.visuals.widgets.active.fg_stroke = on_critical;

        ui.add_sized([BTN_WIDTH, BTN_HEIGHT], egui::Button::new(text))
    })
    .inner
}

fn prune_credential_inputs(inputs: &mut HashMap<String, String>, snapshot: &Snapshot) {
    let still_pending: std::collections::HashSet<&str> = snapshot
        .pending_credentials
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    inputs.retain(|id, _| still_pending.contains(id.as_str()));
}

fn position_top_right(monitor: egui::Vec2) -> egui::Pos2 {
    egui::Pos2::new(
        monitor.x - WINDOW_WIDTH - SCREEN_EDGE_MARGIN,
        SCREEN_EDGE_MARGIN,
    )
}

fn top_right_position(ctx: &egui::Context) -> Option<egui::Pos2> {
    let monitor = ctx.input(|i| i.viewport().monitor_size)?;
    Some(position_top_right(monitor))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityTransition {
    Show,
    Hide,
    Unchanged,
}

fn visibility_transition(should_show: bool, last_visible: bool) -> VisibilityTransition {
    match (should_show, last_visible) {
        (true, false) => VisibilityTransition::Show,
        (false, true) => VisibilityTransition::Hide,
        _ => VisibilityTransition::Unchanged,
    }
}

#[cfg(target_os = "macos")]
fn install_activation_policy(opts: &mut eframe::NativeOptions) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    opts.event_loop_builder = Some(Box::new(|builder| {
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }));
}

#[cfg(not(target_os = "macos"))]
fn install_activation_policy(_opts: &mut eframe::NativeOptions) {}

fn load_icon() -> anyhow::Result<Icon> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/lensTemplate@2x.png");
    let decoder = png::Decoder::new(ICON_BYTES);
    let mut reader = decoder.read_info().context("read embedded icon PNG info")?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .context("decode embedded icon PNG frame")?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 0xFF])
            .collect(),
        other => anyhow::bail!("unsupported PNG color type for tray icon: {other:?}"),
    };
    Icon::from_rgba(rgba, info.width, info.height).context("build Icon from RGBA")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_successfully() {
        let icon = load_icon().expect("embedded template PNG should decode");
        drop(icon);
    }

    #[test]
    fn quit_accelerator_constructs() {
        let accel = Accelerator::new(Some(Modifiers::META), Code::KeyQ);
        let repr = format!("{accel:?}");
        assert!(repr.contains("KeyQ"), "expected KeyQ in {repr}");
    }

    fn credential_prompt(id: &str) -> CredentialCardPrompt {
        CredentialCardPrompt {
            id: id.to_string(),
            credential_id: "cred".to_string(),
            action: "read".to_string(),
            host_value_available: false,
        }
    }

    #[test]
    fn prune_credential_inputs_drops_buffers_for_no_longer_pending_prompts() {
        let snapshot = Snapshot {
            pending: Vec::new(),
            pending_credentials: vec![credential_prompt("keep")],
            informs: Vec::new(),
        };
        let mut inputs = HashMap::new();
        inputs.insert("keep".to_string(), "typed so far".to_string());
        inputs.insert("stale".to_string(), "abandoned".to_string());

        prune_credential_inputs(&mut inputs, &snapshot);

        assert_eq!(inputs.get("keep").map(String::as_str), Some("typed so far"));
        assert!(
            !inputs.contains_key("stale"),
            "stale buffer must be dropped"
        );
    }

    #[test]
    fn position_top_right_insets_the_window_from_the_monitor_corner() {
        let pos = position_top_right(egui::Vec2::new(1920.0, 1080.0));
        assert_eq!(pos.x, 1920.0 - WINDOW_WIDTH - SCREEN_EDGE_MARGIN);
        assert_eq!(pos.y, SCREEN_EDGE_MARGIN);
    }

    #[test]
    fn visibility_transition_show_when_not_yet_visible() {
        assert_eq!(
            visibility_transition(true, false),
            VisibilityTransition::Show
        );
    }

    #[test]
    fn visibility_transition_hide_when_currently_visible() {
        assert_eq!(
            visibility_transition(false, true),
            VisibilityTransition::Hide
        );
    }

    #[test]
    fn visibility_transition_unchanged_when_already_showing() {
        assert_eq!(
            visibility_transition(true, true),
            VisibilityTransition::Unchanged
        );
    }

    #[test]
    fn visibility_transition_unchanged_when_already_hidden() {
        assert_eq!(
            visibility_transition(false, false),
            VisibilityTransition::Unchanged
        );
    }
}
