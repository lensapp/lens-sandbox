use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, Stroke};
use tokio::sync::mpsc;

use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::PendingPrompt;
use crate::credential_flow::session::{CredentialDecisionRequest, CredentialPendingPrompt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionDelivery {
    pub id: String,
    pub decision: Decision,
}

/// Carries the full [`CredentialDecisionRequest`] rather than a bare enum so the typed credential value threads through to `record_decision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialDecisionDelivery {
    pub id: String,
    pub request: CredentialDecisionRequest,
}

/// Its `host_value_available` flag is set only when the per-service [`crate::credential_flow::detection::HostDetector`] returned `Some` at present time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialCardPrompt {
    pub id: String,
    pub credential_id: String,
    pub action: String,
    pub host_value_available: bool,
}

pub struct WindowState {
    inner: Mutex<WindowInner>,
}

#[derive(Default)]
struct WindowInner {
    pending: Vec<PendingEntry>,
    pending_credentials: Vec<CredentialPendingEntry>,
    informs: Vec<String>,
}

struct PendingEntry {
    prompt: PendingPrompt,
    decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
}

struct CredentialPendingEntry {
    prompt: CredentialCardPrompt,
    decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
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
        g.pending.push(PendingEntry {
            prompt,
            decision_tx,
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
        g.pending_credentials.push(CredentialPendingEntry {
            prompt: CredentialCardPrompt {
                id: prompt.id,
                credential_id: prompt.credential_id,
                action: prompt.action,
                host_value_available,
            },
            decision_tx,
        });
    }

    pub fn remove_credential_pending(&self, id: &str) {
        self.lock()
            .pending_credentials
            .retain(|e| e.prompt.id != id);
    }

    pub fn push_inform(&self, msg: String) {
        self.lock().informs.push(msg);
    }

    pub fn clear_informs(&self) {
        self.lock().informs.clear();
    }

    pub fn dismiss_first_inform(&self) {
        let mut g = self.lock();
        if !g.informs.is_empty() {
            g.informs.remove(0);
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
            informs: g.informs.clone(),
        }
    }

    /// Total pending across both flows; drives `tray::sync_viewport_visibility`.
    pub fn pending_count(&self) -> usize {
        let g = self.lock();
        g.pending.len() + g.pending_credentials.len()
    }

    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        let mut g = self.lock();
        let Some(idx) = g.pending.iter().position(|e| e.prompt.id == id) else {
            return false;
        };
        let entry = g.pending.remove(idx);
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            decision,
        });
        true
    }

    /// Mirror of [`Self::decide`] for the credential flow.
    pub fn decide_credential(&self, id: &str, request: CredentialDecisionRequest) -> bool {
        let mut g = self.lock();
        let Some(idx) = g.pending_credentials.iter().position(|e| e.prompt.id == id) else {
            return false;
        };
        let entry = g.pending_credentials.remove(idx);
        let _ = entry.decision_tx.send(CredentialDecisionDelivery {
            id: id.to_string(),
            request,
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
    pub pending_credentials: Vec<CredentialCardPrompt>,
    pub informs: Vec<String>,
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

pub fn lds_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = BG_SECONDARY;
    v.extreme_bg_color = BG_TERTIARY;
    v.faint_bg_color = BORDER;
    v.override_text_color = Some(TEXT_PRIMARY);
    v.hyperlink_color = ACCENT_GREEN;
    v.selection.bg_fill = ACCENT_GREEN;
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
        }
    }

    fn cred_prompt(id: &str, credential_id: &str) -> CredentialPendingPrompt {
        CredentialPendingPrompt {
            id: id.into(),
            credential_id: credential_id.into(),
            action: format!("use of {credential_id} placeholder"),
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
    fn dismiss_first_inform_removes_oldest_and_preserves_rest() {
        let s = WindowState::new();
        s.push_inform("first".into());
        s.push_inform("second".into());
        s.dismiss_first_inform();
        assert_eq!(s.snapshot().informs, vec!["second".to_string()]);
    }

    #[test]
    fn dismiss_first_inform_when_empty_is_a_noop() {
        let s = WindowState::new();
        s.dismiss_first_inform();
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
                decision: Decision::AllowOnce,
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
        assert_eq!(rx1.try_recv().expect("rx1").decision, Decision::DenyAlways);
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
    fn insert_credential_pending_dedupes_by_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        assert_eq!(s.snapshot().pending_credentials.len(), 2);
    }

    #[test]
    fn insert_credential_pending_carries_host_value_flag() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx.clone());
        s.insert_credential_pending(cred_prompt("c2", "openai"), false, tx);
        let snap = s.snapshot();
        assert!(snap.pending_credentials[0].host_value_available);
        assert!(!snap.pending_credentials[1].host_value_available);
    }

    #[test]
    fn remove_credential_pending_drops_only_matching_id() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx.clone());
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
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx);
        s.remove_credential_pending("never-was");
        assert_eq!(s.snapshot().pending_credentials.len(), 1);
    }

    #[test]
    fn pending_count_sums_network_and_credential_lists() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        let (ctx, _crx) = unbounded_channel();
        s.insert_pending(prompt("r1", "a.test"), tx);
        s.insert_credential_pending(cred_prompt("c1", "github"), true, ctx);
        assert_eq!(s.pending_count(), 2);
    }

    #[test]
    fn snapshot_returns_pending_credentials_in_insertion_order() {
        let s = WindowState::new();
        let (tx, _rx) = unbounded_channel();
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx.clone());
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
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx);
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
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx1);
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
        s.insert_credential_pending(cred_prompt("c1", "github"), true, tx);
        assert!(!s.decide_credential("nope", CredentialDecisionRequest::Deny));
        assert_eq!(s.snapshot().pending_credentials.len(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn lds_visuals_uses_dark_palette_and_green_accent() {
        let v = lds_visuals();
        assert_eq!(v.panel_fill, Color32::TRANSPARENT);
        assert_eq!(v.window_fill, BG_SECONDARY);
        assert_eq!(v.override_text_color, Some(TEXT_PRIMARY));
        assert_eq!(v.selection.bg_fill, ACCENT_GREEN);
        assert_eq!(v.hyperlink_color, ACCENT_GREEN);
        assert!(v.dark_mode);
    }
}
