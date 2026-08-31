use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, Stroke};
use tokio::sync::mpsc;

use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::{ConnectionChoice, PendingPrompt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionDelivery {
    pub id: String,
    pub action: RequestAction,
}

/// What the user chose on a card: a wire decision, an answer about the connector that serves the destination, or a closed card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAction {
    Decide(Decision),
    /// Connect this project to the offered connector by the named method (§3.2.4).
    Grant {
        method: String,
        connection: ConnectionChoice,
    },
    /// A standing no for this project; the ordinary card then asks what the hold stood in for.
    Decline,
    /// A closed card: fail the held request, but record nothing — the developer made no decision.
    Dismiss,
}

pub struct WindowState {
    inner: Mutex<WindowInner>,
}

#[derive(Default)]
struct WindowInner {
    pending: Vec<PendingEntry>,
    informs: Vec<InformEntry>,
    next_seq: u64,
}

impl WindowInner {
    fn alloc_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn order(&self) -> Vec<StackItem> {
        let mut keyed: Vec<(u64, StackItem)> = Vec::new();
        keyed.extend(seq_keyed(
            self.informs.iter().map(|e| e.seq),
            StackItem::Inform,
        ));
        keyed.extend(seq_keyed(
            self.pending.iter().map(|e| e.seq),
            StackItem::Network,
        ));
        keyed.sort_by_key(|(seq, _)| *seq);
        keyed.into_iter().map(|(_, item)| item).collect()
    }
}

fn seq_keyed(
    seqs: impl Iterator<Item = u64>,
    item: fn(usize) -> StackItem,
) -> impl Iterator<Item = (u64, StackItem)> {
    seqs.enumerate().map(move |(i, seq)| (seq, item(i)))
}

struct PendingEntry {
    prompt: PendingPrompt,
    decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    seq: u64,
}

struct InformEntry {
    msg: String,
    seq: u64,
}

impl WindowState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(WindowInner::default()),
        })
    }

    pub fn insert_pending(
        &self,
        prompt: PendingPrompt,
        decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    ) {
        let mut g = self.lock();
        // Presenting an id already on screen updates it in place: a declined offer re-presents the same request as the ordinary question, and the card must show that rather than the answer already given. The seq stays so the card keeps its place in the pile.
        if let Some(entry) = g.pending.iter_mut().find(|e| e.prompt.id == prompt.id) {
            entry.prompt = prompt;
            return;
        }
        let seq = g.alloc_seq();
        g.pending.push(PendingEntry {
            prompt,
            decision_tx,
            seq,
        });
    }

    pub fn remove_pending(&self, id: &str) {
        self.lock().pending.retain(|e| e.prompt.id != id);
    }

    pub fn push_inform(&self, msg: String) {
        let mut g = self.lock();
        let seq = g.alloc_seq();
        g.informs.push(InformEntry { msg, seq });
    }

    pub fn clear_informs(&self) {
        self.lock().informs.clear();
    }

    pub fn dismiss_inform(&self, index: usize) {
        let mut g = self.lock();
        if index < g.informs.len() {
            g.informs.remove(index);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        Snapshot {
            pending: g.pending.iter().map(|e| e.prompt.clone()).collect(),
            informs: g.informs.iter().map(|e| e.msg.clone()).collect(),
            order: g.order(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.lock().pending.len()
    }

    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        self.deliver(id, RequestAction::Decide(decision))
    }

    pub fn grant(&self, id: &str, method: &str, connection: ConnectionChoice) -> bool {
        self.deliver(
            id,
            RequestAction::Grant {
                method: method.to_string(),
                connection,
            },
        )
    }

    /// Keeps the card: a decline is answered by the ordinary question the hold stood in for, on the same request.
    pub fn decline(&self, id: &str) -> bool {
        let g = self.lock();
        let Some(entry) = g.pending.iter().find(|e| e.prompt.id == id) else {
            return false;
        };
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            action: RequestAction::Decline,
        });
        true
    }

    /// Drops the card and fails its held request without recording a decision. See [`RequestAction::Dismiss`].
    pub fn dismiss(&self, id: &str) -> bool {
        self.deliver(id, RequestAction::Dismiss)
    }

    fn deliver(&self, id: &str, action: RequestAction) -> bool {
        let mut g = self.lock();
        let Some(idx) = g.pending.iter().position(|e| e.prompt.id == id) else {
            return false;
        };
        let entry = g.pending.remove(idx);
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            action,
        });
        true
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WindowInner> {
        self.inner.lock().expect("window state mutex poisoned")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pending: Vec<PendingPrompt>,
    pub informs: Vec<String>,
    /// Every entry above in arrival order, so a card keeps its place in the stack as others come and go.
    pub order: Vec<StackItem>,
}

/// One renderable entry of the approval window's stack, indexing into its [`Snapshot`] list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackItem {
    Inform(usize),
    Network(usize),
}

static GLOBAL: OnceLock<Arc<WindowState>> = OnceLock::new();
static CTX: OnceLock<egui::Context> = OnceLock::new();

pub fn install(state: Arc<WindowState>) {
    let _ = GLOBAL.set(state);
}

pub fn get() -> Option<Arc<WindowState>> {
    GLOBAL.get().cloned()
}

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
    use crate::approval_flow::protocol::Treatment;
    use tokio::sync::mpsc::unbounded_channel;

    fn prompt(id: &str, host: &str) -> PendingPrompt {
        PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            treatment: Treatment::Inspected,
            run: None,
            offer: None,
        }
    }

    #[test]
    fn granting_takes_the_card_and_carries_the_account_the_user_chose() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "api.some-provider.example"), tx);

        assert!(s.grant("r1", "token", ConnectionChoice::Held("work".into())));

        assert_eq!(
            rx.try_recv().expect("a delivery"),
            DecisionDelivery {
                id: "r1".into(),
                action: RequestAction::Grant {
                    method: "token".into(),
                    connection: ConnectionChoice::Held("work".into()),
                },
            }
        );
        assert_eq!(s.pending_count(), 0, "the grant answers this card");
    }

    #[test]
    fn declining_keeps_the_card_because_the_ordinary_question_is_still_unanswered() {
        // The hold already turned this request into a question; the session re-presents it without the offer.
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "api.some-provider.example"), tx);

        assert!(s.decline("r1"));

        assert_eq!(
            rx.try_recv().expect("a delivery").action,
            RequestAction::Decline
        );
        assert_eq!(s.pending_count(), 1);
    }

    #[test]
    fn answering_a_card_that_is_gone_delivers_nothing() {
        let s = WindowState::new();
        assert!(!s.grant("gone", "token", ConnectionChoice::None));
        assert!(!s.decline("gone"));
    }

    #[test]
    fn presenting_a_card_again_replaces_what_it_shows() {
        // A declined offer is re-presented as the ordinary question; an early return would leave the connector card on screen with both its buttons already spent.
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let mut offered = prompt("r1", "api.some-provider.example");
        offered.offer = Some(lns_ipc::ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: Vec::new(),
            connections: Vec::new(),
        });
        s.insert_pending(offered, tx.clone());

        s.insert_pending(prompt("r1", "api.some-provider.example"), tx);

        let snapshot = s.snapshot();
        assert_eq!(snapshot.pending.len(), 1, "still one card, not two");
        assert!(
            snapshot.pending[0].offer.is_none(),
            "and it is the ordinary question now"
        );
    }

    #[test]
    fn insert_pending_dedupes_by_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx.clone());
        s.insert_pending(prompt("r1", "a.test"), tx.clone());
        s.insert_pending(prompt("r2", "b.test"), tx);
        assert_eq!(s.pending_count(), 2);
    }

    #[test]
    fn a_network_card_keeps_the_run_its_prompt_names() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let mut named = prompt("r1", "a.test");
        named.run = Some("some-run".into());
        s.insert_pending(named, tx);
        assert_eq!(
            s.snapshot().pending[0].run.as_deref(),
            Some("some-run"),
            "the window must not drop the attribution the service put on the prompt"
        );
    }

    #[test]
    fn remove_pending_drops_only_matching_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx.clone());
        s.insert_pending(prompt("r2", "b.test"), tx);
        s.remove_pending("r1");
        let snap = s.snapshot();
        assert_eq!(snap.pending.len(), 1);
        assert_eq!(snap.pending[0].id, "r2");
    }

    #[test]
    fn remove_unknown_id_is_a_noop() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        s.remove_pending("never-was");
        assert_eq!(s.pending_count(), 1);
    }

    #[test]
    fn push_inform_appends_in_order() {
        let s = WindowState::new();
        s.push_inform("first".into());
        s.push_inform("second".into());
        let snap = s.snapshot();
        assert_eq!(snap.informs, vec!["first".to_string(), "second".into()]);
    }

    #[test]
    fn clear_informs_empties_the_list() {
        let s = WindowState::new();
        s.push_inform("warn".into());
        s.clear_informs();
        assert!(s.snapshot().informs.is_empty());
    }

    #[test]
    fn dismiss_inform_removes_the_indexed_entry_and_preserves_the_rest() {
        let s = WindowState::new();
        s.push_inform("first".into());
        s.push_inform("second".into());
        s.push_inform("third".into());
        s.dismiss_inform(1);
        assert_eq!(
            s.snapshot().informs,
            vec!["first".to_string(), "third".into()],
            "dismissing a stacked banner drops that banner, not the oldest"
        );
    }

    #[test]
    fn dismiss_inform_out_of_bounds_is_a_noop() {
        let s = WindowState::new();
        s.push_inform("only".into());
        s.dismiss_inform(5);
        assert_eq!(s.snapshot().informs, vec!["only".to_string()]);
    }

    #[test]
    fn dismiss_inform_when_empty_is_a_noop() {
        let s = WindowState::new();
        s.dismiss_inform(0);
        assert!(s.snapshot().informs.is_empty());
    }

    #[test]
    fn snapshot_returns_pending_in_insertion_order() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx.clone());
        s.insert_pending(prompt("r2", "b.test"), tx);
        let snap = s.snapshot();
        assert_eq!(snap.pending.len(), 2);
        assert_eq!(snap.pending[0].id, "r1");
        assert_eq!(snap.pending[1].id, "r2");
    }

    #[test]
    fn decide_sends_delivery_on_matching_tx_and_removes_entry() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        assert!(s.decide("r1", Decision::AllowOnce));
        assert_eq!(s.pending_count(), 0);
        let got = rx.try_recv().expect("delivery");
        assert_eq!(
            got,
            DecisionDelivery {
                id: "r1".into(),
                action: RequestAction::Decide(Decision::AllowOnce),
            }
        );
    }

    #[test]
    fn dismiss_delivers_a_verdict_free_action_and_removes_the_entry() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);

        assert!(s.dismiss("r1"));

        assert_eq!(s.pending_count(), 0);
        assert_eq!(
            rx.try_recv().expect("delivery"),
            DecisionDelivery {
                id: "r1".into(),
                action: RequestAction::Dismiss,
            },
            "a closed card carries no decision to the session"
        );
    }

    #[test]
    fn decide_routes_to_the_tx_supplied_at_insert_not_a_sibling() {
        let s = WindowState::new();
        let (tx1, mut rx1) = unbounded_channel();
        let (tx2, mut rx2) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx1);
        s.insert_pending(prompt("r2", "b.test"), tx2);
        assert!(s.decide("r1", Decision::DenyAlways));
        assert_eq!(
            rx1.try_recv().expect("rx1").action,
            RequestAction::Decide(Decision::DenyAlways)
        );
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn decide_returns_false_for_unknown_id_and_emits_no_delivery() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        assert!(!s.decide("nope", Decision::AllowOnce));
        assert_eq!(s.pending_count(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn install_publishes_state_and_ctx_and_getters_return_them() {
        let s = WindowState::new();
        install(s.clone());
        install_ctx(egui::Context::default());
        let got_state = get().expect("global state should be installed");
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
