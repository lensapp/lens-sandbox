use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, Stroke};
use tokio::sync::mpsc;

use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::PendingPrompt;
use crate::credential_flow::session::{CredentialDecisionRequest, CredentialPendingPrompt};
use crate::credential_flow::store::CredentialEntry;
use crate::oauth::SignInPivot;
use lns_policy::integrations::TokenFallback;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionDelivery {
    pub id: String,
    pub action: RequestAction,
}

/// What the user chose on a network card: one of the wire decisions, accepting the integration offer via its interactive connect, or connecting it with a pasted token. The latter two are host-only actions that drive a connect rather than a per-request verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAction {
    Decide(Decision),
    ConnectIntegration,
    UseToken { value: String },
}

/// Carries the full [`CredentialDecisionRequest`] rather than a bare enum so the typed credential value threads through to `record_decision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialDecisionDelivery {
    pub id: String,
    pub request: CredentialDecisionRequest,
}

/// Its `host_value_available` flag is set only when the per-service [`crate::credential_flow::detection::HostDetector`] returned `Some` at present time; `oauth_display_name` is `Some` for an oauth integration, making the card a browser-sign-in consent rather than a value prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialCardPrompt {
    pub id: String,
    pub credential_id: String,
    pub action: String,
    pub host_value_available: bool,
    pub oauth_display_name: Option<String>,
    pub token_fallback: Option<TokenFallback>,
}

/// An interactive sign-in card: which service, where to sign in, and — for a device flow — the code to type (`None` for a pkce browser redirect). `token_fallback` is `Some` when the integration lets a blocked user pivot to a pasted token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInCard {
    pub credential_id: String,
    pub display_name: String,
    pub user_code: Option<String>,
    pub verification_uri: String,
    pub token_fallback: Option<TokenFallback>,
}

pub struct WindowState {
    inner: Mutex<WindowInner>,
}

#[derive(Default)]
struct WindowInner {
    pending: Vec<PendingEntry>,
    pending_credentials: Vec<CredentialPendingEntry>,
    pending_sign_ins: Vec<SignInEntry>,
    informs: Vec<InformEntry>,
    connecting: Vec<ConnectingEntry>,
    next_seq: u64,
}

impl WindowInner {
    fn alloc_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn take_connecting(&mut self, display_name: &str) -> Option<u64> {
        let idx = self
            .connecting
            .iter()
            .position(|e| e.display_name == display_name)?;
        Some(self.connecting.remove(idx).seq)
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
        keyed.extend(seq_keyed(
            self.pending_sign_ins.iter().map(|e| e.seq),
            StackItem::SignIn,
        ));
        keyed.extend(seq_keyed(
            self.pending_credentials.iter().map(|e| e.seq),
            StackItem::Credential,
        ));
        keyed.extend(seq_keyed(
            self.connecting.iter().map(|e| e.seq),
            StackItem::Connecting,
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

struct CredentialPendingEntry {
    prompt: CredentialCardPrompt,
    decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
    seq: u64,
}

struct SignInEntry {
    card: SignInCard,
    cancel: tokio::sync::oneshot::Sender<SignInPivot>,
    seq: u64,
}

struct InformEntry {
    msg: String,
    seq: u64,
}

/// Holds an accepted offer's slot in the stack while its connect is in flight, so the card doesn't vanish and re-emerge somewhere else.
struct ConnectingEntry {
    display_name: String,
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
        if g.pending.iter().any(|e| e.prompt.id == prompt.id) {
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

    /// Duplicate ids are silently coalesced (dedup invariant S11).
    pub fn insert_credential_pending(
        &self,
        prompt: CredentialPendingPrompt,
        host_value_available: bool,
        decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
    ) {
        let mut g = self.lock();
        if g.pending_credentials
            .iter()
            .any(|e| e.prompt.id == prompt.id)
        {
            return;
        }
        let seq = g.alloc_seq();
        g.pending_credentials.push(CredentialPendingEntry {
            prompt: CredentialCardPrompt {
                id: prompt.id,
                credential_id: prompt.credential_id,
                action: prompt.action,
                host_value_available,
                oauth_display_name: prompt.oauth_display_name,
                token_fallback: prompt.token_fallback,
            },
            decision_tx,
            seq,
        });
    }

    pub fn remove_credential_pending(&self, id: &str) {
        self.lock()
            .pending_credentials
            .retain(|e| e.prompt.id != id);
    }

    /// Coalesces by `credential_id`; the `cancel` sender aborts the in-flight device-flow poll (or pivots it to a pasted token) when the card resolves. The card takes over a matching connecting placeholder's slot so it appears where the accepted card was.
    pub fn insert_sign_in(
        &self,
        card: SignInCard,
        cancel: tokio::sync::oneshot::Sender<SignInPivot>,
    ) {
        let mut g = self.lock();
        if g.pending_sign_ins
            .iter()
            .any(|e| e.card.credential_id == card.credential_id)
        {
            return;
        }
        let seq = g
            .take_connecting(&card.display_name)
            .unwrap_or_else(|| g.alloc_seq());
        g.pending_sign_ins.push(SignInEntry { card, cancel, seq });
    }

    pub fn remove_sign_in(&self, credential_id: &str) {
        self.lock()
            .pending_sign_ins
            .retain(|e| e.card.credential_id != credential_id);
    }

    /// Drops the card and aborts the in-flight device-flow poll; returns whether a card was present.
    pub fn cancel_sign_in(&self, credential_id: &str) -> bool {
        self.resolve_sign_in(credential_id, SignInPivot::Cancel)
    }

    /// Drops the card and pivots the in-flight device flow to the pasted token, so the held request is released without restarting the connect.
    pub fn pivot_sign_in(&self, credential_id: &str, value: String) -> bool {
        self.resolve_sign_in(credential_id, SignInPivot::UseToken(value))
    }

    /// Drops the sign-in card and fires its cancel channel with `pivot`; returns whether a card was present.
    fn resolve_sign_in(&self, credential_id: &str, pivot: SignInPivot) -> bool {
        let mut g = self.lock();
        let Some(idx) = g
            .pending_sign_ins
            .iter()
            .position(|e| e.card.credential_id == credential_id)
        else {
            return false;
        };
        let entry = g.pending_sign_ins.remove(idx);
        let _ = entry.cancel.send(pivot);
        true
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
            pending_credentials: g
                .pending_credentials
                .iter()
                .map(|e| e.prompt.clone())
                .collect(),
            sign_ins: g.pending_sign_ins.iter().map(|e| e.card.clone()).collect(),
            informs: g.informs.iter().map(|e| e.msg.clone()).collect(),
            connecting: g
                .connecting
                .iter()
                .map(|e| e.display_name.clone())
                .collect(),
            order: g.order(),
        }
    }

    /// Total pending across every flow.
    pub fn pending_count(&self) -> usize {
        let g = self.lock();
        g.pending.len() + g.pending_credentials.len() + g.pending_sign_ins.len()
    }

    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        self.deliver(id, RequestAction::Decide(decision))
    }

    /// Accepts the integration offer via its interactive connect (browser sign-in or straight credential connect). See [`Self::route_offer`].
    pub fn connect_offer(&self, id: &str) -> bool {
        self.route_offer(id, RequestAction::ConnectIntegration)
    }

    /// Accepts the integration offer by connecting it with a pasted token. See [`Self::route_offer`].
    pub fn use_offer_token(&self, id: &str, value: String) -> bool {
        self.route_offer(id, RequestAction::UseToken { value })
    }

    /// Routes one connect action for the clicked request and immediately drops every other offer card for the same integration, so no sibling card flashes up before the in-flight connect releases them all. The clicked card's slot is kept by a connecting placeholder until the connect resolves.
    fn route_offer(&self, id: &str, action: RequestAction) -> bool {
        let mut g = self.lock();
        let Some(idx) = g.pending.iter().position(|e| e.prompt.id == id) else {
            return false;
        };
        let offer = g.pending[idx].prompt.offer.clone();
        let entry = g.pending.remove(idx);
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            action,
        });
        if let Some(name) = offer {
            g.pending
                .retain(|e| e.prompt.offer.as_deref() != Some(name.as_str()));
            g.connecting.push(ConnectingEntry {
                display_name: name,
                seq: entry.seq,
            });
        }
        true
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

    /// Declines the integration offer: clears the offer on every held request for that integration so each falls back to the plain allow/deny (rather than re-offering via a sibling card) without resolving them; returns whether an offer was present to clear.
    pub fn decline_offer(&self, id: &str) -> bool {
        let mut g = self.lock();
        let Some(name) = g
            .pending
            .iter()
            .find(|e| e.prompt.id == id)
            .and_then(|e| e.prompt.offer.clone())
        else {
            return false;
        };
        for entry in g
            .pending
            .iter_mut()
            .filter(|e| e.prompt.offer.as_deref() == Some(name.as_str()))
        {
            entry.prompt.offer = None;
        }
        true
    }

    /// Mirror of [`Self::decide`] for the credential flow. A browser-consent accept on an oauth card keeps the card's slot via a connecting placeholder, because its device sign-in arrives asynchronously.
    pub fn decide_credential(&self, id: &str, request: CredentialDecisionRequest) -> bool {
        let mut g = self.lock();
        let Some(idx) = g.pending_credentials.iter().position(|e| e.prompt.id == id) else {
            return false;
        };
        let entry = g.pending_credentials.remove(idx);
        if let Some(name) = consent_connect_name(&entry.prompt, &request) {
            g.connecting.push(ConnectingEntry {
                display_name: name,
                seq: entry.seq,
            });
        }
        let _ = entry.decision_tx.send(CredentialDecisionDelivery {
            id: id.to_string(),
            request,
        });
        true
    }

    /// Drops every connecting placeholder for `display_name` once its connect has resolved.
    pub fn clear_connecting(&self, display_name: &str) {
        self.lock()
            .connecting
            .retain(|e| e.display_name != display_name);
    }

    /// Drops every connecting placeholder regardless of integration; used on run teardown so an in-flight connect can't keep the window pinned after the workload is gone.
    pub fn clear_all_connecting(&self) {
        self.lock().connecting.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WindowInner> {
        self.inner.lock().expect("window state mutex poisoned")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pending: Vec<PendingPrompt>,
    pub pending_credentials: Vec<CredentialCardPrompt>,
    pub sign_ins: Vec<SignInCard>,
    pub informs: Vec<String>,
    /// Display names of integrations whose accepted connect is still in flight, each holding its card's slot.
    pub connecting: Vec<String>,
    /// Every entry above in arrival order, so a card keeps its place in the stack as others come and go.
    pub order: Vec<StackItem>,
}

/// One renderable entry of the approval window's stack, indexing into its [`Snapshot`] list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackItem {
    Inform(usize),
    Network(usize),
    SignIn(usize),
    Credential(usize),
    Connecting(usize),
}

/// The placeholder name for a consent-card decision that drives an asynchronous device sign-in — a browser-consent accept on an oauth card; a pasted token or a deny resolves synchronously and holds no slot.
fn consent_connect_name(
    prompt: &CredentialCardPrompt,
    request: &CredentialDecisionRequest,
) -> Option<String> {
    let name = prompt.oauth_display_name.as_ref()?;
    match request {
        CredentialDecisionRequest::Allow(CredentialEntry::Stored { .. }) => None,
        CredentialDecisionRequest::Allow(_) => Some(name.clone()),
        _ => None,
    }
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
    v.selection.stroke = Stroke::new(1.0, TEXT_ACCENT);

    let radius = egui::CornerRadius::same(8);

    v.widgets.noninteractive.bg_fill = BG_SECONDARY;
    v.widgets.noninteractive.weak_bg_fill = BG_SECONDARY;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v.widgets.noninteractive.corner_radius = radius;

    v.widgets.inactive.bg_fill = ACCENT_GREEN;
    v.widgets.inactive.weak_bg_fill = ACCENT_GREEN;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, BG_PRIMARY);
    v.widgets.inactive.corner_radius = radius;

    v.widgets.hovered.bg_fill = ACCENT_GREEN_HOVER;
    v.widgets.hovered.weak_bg_fill = ACCENT_GREEN_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_GREEN_HOVER);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, BG_PRIMARY);
    v.widgets.hovered.corner_radius = radius;

    v.widgets.active.bg_fill = ACCENT_GREEN_PRESSED;
    v.widgets.active.weak_bg_fill = ACCENT_GREEN_PRESSED;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT_GREEN_PRESSED);
    v.widgets.active.fg_stroke = Stroke::new(1.0, BG_PRIMARY);
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
    use crate::credential_flow::store::CredentialEntry;
    use tokio::sync::mpsc::unbounded_channel;

    fn prompt(id: &str, host: &str) -> PendingPrompt {
        PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            offer: None,
            token_fallback: None,
        }
    }

    fn cred_prompt(id: &str, credential_id: &str) -> CredentialPendingPrompt {
        CredentialPendingPrompt {
            id: id.into(),
            credential_id: credential_id.into(),
            action: format!("use of {credential_id} placeholder"),
            oauth_display_name: None,
            token_fallback: None,
        }
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

    fn offer_prompt(id: &str, host: &str, name: &str) -> PendingPrompt {
        PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            offer: Some(name.into()),
            token_fallback: None,
        }
    }

    #[test]
    fn connect_offer_drops_the_card_and_routes_a_connect_action() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.connect_offer("r1"));
        assert_eq!(s.pending_count(), 0, "accepting the offer clears the card");
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "r1");
        assert_eq!(got.action, RequestAction::ConnectIntegration);
    }

    #[test]
    fn connect_offer_hides_every_sibling_offer_card_synchronously() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(offer_prompt("r2", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.connect_offer("r1"));
        assert_eq!(
            s.pending_count(),
            0,
            "the clicked card and its siblings vanish on the same frame, so none flashes up"
        );
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "r1");
        assert_eq!(got.action, RequestAction::ConnectIntegration);
        assert!(
            rx.try_recv().is_err(),
            "only one connect is routed; the in-flight connect releases the siblings"
        );
    }

    #[test]
    fn connect_offer_leaves_unrelated_cards_pending() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(prompt("r2", "example.com"), tx);
        assert!(s.connect_offer("r1"));
        let snap = s.snapshot();
        assert_eq!(snap.pending.len(), 1);
        assert_eq!(
            snap.pending[0].id, "r2",
            "an unrelated plain request stays pending"
        );
    }

    #[test]
    fn connect_offer_returns_false_for_unknown_id() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(!s.connect_offer("nope"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn use_offer_token_routes_a_use_token_action_and_drops_sibling_offer_cards() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(offer_prompt("r2", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.use_offer_token("r1", "some-pasted-token".into()));
        assert_eq!(
            s.pending_count(),
            0,
            "the clicked card and its siblings vanish so none flashes up before the connect releases them"
        );
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "r1");
        assert_eq!(
            got.action,
            RequestAction::UseToken {
                value: "some-pasted-token".into()
            }
        );
        assert!(rx.try_recv().is_err(), "only one connect is routed");
    }

    #[test]
    fn use_offer_token_returns_false_for_unknown_id() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(!s.use_offer_token("nope", "some-pasted-token".into()));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn decline_offer_clears_the_offer_and_keeps_the_request_pending() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.decline_offer("r1"));
        let snap = s.snapshot();
        assert_eq!(
            snap.pending.len(),
            1,
            "declining keeps the request for a plain decision"
        );
        assert_eq!(
            snap.pending[0].offer, None,
            "the offer is cleared so the network card shows next"
        );
    }

    #[test]
    fn decline_offer_clears_every_card_for_the_same_integration() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(offer_prompt("r2", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.decline_offer("r1"));
        let snap = s.snapshot();
        assert!(
            snap.pending.iter().all(|p| p.offer.is_none()),
            "declining the offer clears it for every held GitHub request, not just the clicked one"
        );
        assert_eq!(
            snap.pending.len(),
            2,
            "the requests stay pending as plain cards"
        );
    }

    #[test]
    fn decline_offer_is_false_when_there_is_no_offer_to_clear() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "example.com"), tx);
        assert!(
            !s.decline_offer("r1"),
            "a plain request has no offer to decline"
        );
        assert!(
            !s.decline_offer("nope"),
            "an unknown id has no offer to decline"
        );
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
    fn insert_credential_pending_dedupes_by_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        assert_eq!(s.snapshot().pending_credentials.len(), 2);
    }

    #[test]
    fn insert_credential_pending_carries_host_value_flag() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        let snap = s.snapshot();
        assert!(snap.pending_credentials[0].host_value_available);
        assert!(!snap.pending_credentials[1].host_value_available);
    }

    #[test]
    fn insert_credential_pending_carries_the_token_fallback() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let mut prompt = cred_prompt("c1", "some-oauth");
        prompt.token_fallback = Some(TokenFallback {
            help: Some("https://example.com/pat".into()),
        });
        s.insert_credential_pending(prompt, false, tx);
        assert_eq!(
            s.snapshot().pending_credentials[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
            }),
            "the card carries the prompt's token fallback so the consent card can offer the pivot"
        );
    }

    #[test]
    fn remove_credential_pending_drops_only_matching_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        s.remove_credential_pending("c1");
        let snap = s.snapshot();
        assert_eq!(snap.pending_credentials.len(), 1);
        assert_eq!(snap.pending_credentials[0].id, "c2");
    }

    #[test]
    fn remove_credential_unknown_id_is_a_noop() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx);
        s.remove_credential_pending("never-was");
        assert_eq!(s.snapshot().pending_credentials.len(), 1);
    }

    #[test]
    fn pending_count_sums_network_and_credential_lists() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let (ctx, _crx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, ctx);
        assert_eq!(s.pending_count(), 2);
    }

    #[test]
    fn snapshot_returns_pending_credentials_in_insertion_order() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        let snap = s.snapshot();
        assert_eq!(snap.pending_credentials.len(), 2);
        assert_eq!(snap.pending_credentials[0].id, "c1");
        assert_eq!(snap.pending_credentials[1].id, "c2");
    }

    #[test]
    fn decide_credential_sends_delivery_on_matching_tx_and_removes_entry() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx);
        assert!(s.decide_credential(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        ));
        assert_eq!(s.snapshot().pending_credentials.len(), 0);
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "c1");
        assert_eq!(
            got.request,
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn decide_credential_routes_to_the_tx_supplied_at_insert_not_a_sibling() {
        let s = WindowState::new();
        let (tx1, mut rx1) = unbounded_channel();
        let (tx2, mut rx2) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx1);
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx2);
        assert!(s.decide_credential("c1", CredentialDecisionRequest::Deny));
        assert_eq!(
            rx1.try_recv().expect("rx1").request,
            CredentialDecisionRequest::Deny
        );
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn decide_credential_returns_false_for_unknown_id_and_emits_no_delivery() {
        let s = WindowState::new();
        let (tx, mut rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx);
        assert!(!s.decide_credential("nope", CredentialDecisionRequest::Deny));
        assert_eq!(s.snapshot().pending_credentials.len(), 1);
        assert!(rx.try_recv().is_err());
    }

    fn sign_in_card(credential_id: &str) -> SignInCard {
        SignInCard {
            credential_id: credential_id.into(),
            display_name: "GitHub".into(),
            user_code: Some("WXYZ-1234".into()),
            verification_uri: "https://some-oauth.example/login/device".into(),
            token_fallback: None,
        }
    }

    #[test]
    fn insert_sign_in_shows_in_snapshot_and_counts_toward_pending() {
        let s = WindowState::new();
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        let snap = s.snapshot();
        assert_eq!(snap.sign_ins.len(), 1);
        assert_eq!(snap.sign_ins[0].display_name, "GitHub");
        assert_eq!(snap.sign_ins[0].user_code.as_deref(), Some("WXYZ-1234"));
        assert_eq!(s.pending_count(), 1, "a sign-in card keeps the window up");
    }

    #[test]
    fn insert_sign_in_dedupes_by_credential_id() {
        let s = WindowState::new();
        let (tx1, _r1) = tokio::sync::oneshot::channel();
        let (tx2, _r2) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), tx1);
        s.insert_sign_in(sign_in_card("some-oauth"), tx2);
        assert_eq!(s.snapshot().sign_ins.len(), 1);
    }

    #[test]
    fn remove_sign_in_drops_the_card_without_firing_cancel() {
        let s = WindowState::new();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        s.remove_sign_in("some-oauth");
        assert!(s.snapshot().sign_ins.is_empty());
        // remove drops the sender without sending a cancel, so the receiver observes a closed channel.
        let recv = cancel_rx.try_recv();
        assert_eq!(recv, Err(tokio::sync::oneshot::error::TryRecvError::Closed));
    }

    #[test]
    fn cancel_sign_in_drops_the_card_and_fires_cancel() {
        let s = WindowState::new();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        assert!(s.cancel_sign_in("some-oauth"));
        assert!(s.snapshot().sign_ins.is_empty());
        assert_eq!(cancel_rx.try_recv(), Ok(SignInPivot::Cancel));
    }

    #[test]
    fn cancel_sign_in_for_unknown_id_is_false() {
        let s = WindowState::new();
        assert!(!s.cancel_sign_in("nope"));
    }

    #[test]
    fn pivot_sign_in_drops_the_card_and_fires_the_pasted_token() {
        let s = WindowState::new();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        assert!(s.pivot_sign_in("some-oauth", "some-pasted-token".into()));
        assert!(s.snapshot().sign_ins.is_empty());
        assert_eq!(
            cancel_rx.try_recv(),
            Ok(SignInPivot::UseToken("some-pasted-token".into())),
            "the in-flight device flow receives the pasted token, not a cancel"
        );
    }

    #[test]
    fn pivot_sign_in_for_unknown_id_is_false() {
        let s = WindowState::new();
        assert!(!s.pivot_sign_in("nope", "some-x-token".into()));
    }

    #[test]
    fn snapshot_order_interleaves_kinds_by_arrival() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let (cred_tx, _crx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx.clone());
        s.push_inform("warn".into());
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), false, cred_tx);
        s.insert_pending(prompt("r2", "b.test"), tx);
        assert_eq!(
            s.snapshot().order,
            vec![
                StackItem::Network(0),
                StackItem::Inform(0),
                StackItem::Credential(0),
                StackItem::Network(1),
            ],
            "the stack is ordered by arrival, not regrouped by kind, so cards never jump"
        );
    }

    #[test]
    fn connect_offer_keeps_the_clicked_cards_slot_with_a_connecting_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(prompt("r2", "example.com"), tx);
        assert!(s.connect_offer("r1"));
        let snap = s.snapshot();
        assert_eq!(snap.connecting, vec!["GitHub".to_string()]);
        assert_eq!(
            snap.order,
            vec![StackItem::Connecting(0), StackItem::Network(0)],
            "accepting the offer leaves a placeholder in the card's slot instead of collapsing the stack"
        );
    }

    #[test]
    fn use_offer_token_keeps_the_clicked_cards_slot_with_a_connecting_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.use_offer_token("r1", "some-pasted-token".into()));
        assert_eq!(s.snapshot().connecting, vec!["GitHub".to_string()]);
    }

    #[test]
    fn decline_offer_leaves_no_connecting_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        assert!(s.decline_offer("r1"));
        assert!(s.snapshot().connecting.is_empty());
    }

    #[test]
    fn insert_sign_in_takes_over_a_matching_connecting_placeholders_slot() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(prompt("r2", "example.com"), tx);
        s.connect_offer("r1");
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        let snap = s.snapshot();
        assert!(
            snap.connecting.is_empty(),
            "the sign-in card replaces the placeholder"
        );
        assert_eq!(
            snap.order,
            vec![StackItem::SignIn(0), StackItem::Network(0)],
            "the sign-in card appears where the accepted card was, not underneath the other request"
        );
    }

    #[test]
    fn insert_sign_in_without_a_placeholder_appends_at_the_bottom() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        s.insert_sign_in(sign_in_card("some-oauth"), cancel_tx);
        assert_eq!(
            s.snapshot().order,
            vec![StackItem::Network(0), StackItem::SignIn(0)]
        );
    }

    #[test]
    fn clear_connecting_drops_only_the_named_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(
            offer_prompt("r2", "api.other-oauth.example", "Other Service"),
            tx,
        );
        s.connect_offer("r1");
        s.connect_offer("r2");
        s.clear_connecting("GitHub");
        assert_eq!(s.snapshot().connecting, vec!["Other Service".to_string()]);
    }

    #[test]
    fn clear_connecting_for_an_unknown_name_is_a_noop() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(offer_prompt("r1", "api.some-oauth.example", "GitHub"), tx);
        s.connect_offer("r1");
        s.clear_connecting("Other Service");
        assert_eq!(s.snapshot().connecting, vec!["GitHub".to_string()]);
    }

    #[test]
    fn clear_all_connecting_drops_every_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_pending(
            offer_prompt("r1", "api.some-oauth.example", "GitHub"),
            tx.clone(),
        );
        s.insert_pending(
            offer_prompt("r2", "api.other-oauth.example", "Other Service"),
            tx,
        );
        s.connect_offer("r1");
        s.connect_offer("r2");
        s.clear_all_connecting();
        assert!(
            s.snapshot().connecting.is_empty(),
            "run teardown takes down every in-flight placeholder so the window can close"
        );
    }

    fn oauth_cred_prompt(id: &str, name: &str) -> CredentialPendingPrompt {
        CredentialPendingPrompt {
            id: id.into(),
            credential_id: "some-oauth".into(),
            action: "use of some-oauth placeholder".into(),
            oauth_display_name: Some(name.into()),
            token_fallback: None,
        }
    }

    #[test]
    fn consent_accept_keeps_the_cards_slot_with_a_connecting_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(oauth_cred_prompt("c1", "GitHub"), false, tx);
        assert!(s.decide_credential(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        ));
        let snap = s.snapshot();
        assert_eq!(
            snap.connecting,
            vec!["GitHub".to_string()],
            "the consent card's slot waits for the sign-in card its accept will produce"
        );
        assert_eq!(snap.order, vec![StackItem::Connecting(0)]);
    }

    #[test]
    fn consent_accept_with_a_pasted_token_leaves_no_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(oauth_cred_prompt("c1", "GitHub"), false, tx);
        assert!(s.decide_credential(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-pasted-token".into()
            })
        ));
        assert!(
            s.snapshot().connecting.is_empty(),
            "a pasted token arms synchronously, so no slot needs holding"
        );
    }

    #[test]
    fn consent_deny_leaves_no_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(oauth_cred_prompt("c1", "GitHub"), false, tx);
        assert!(s.decide_credential("c1", CredentialDecisionRequest::Deny));
        assert!(s.snapshot().connecting.is_empty());
    }

    #[test]
    fn plain_credential_accept_leaves_no_placeholder() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "some-provider"), true, tx);
        assert!(s.decide_credential(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        ));
        assert!(s.snapshot().connecting.is_empty());
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
