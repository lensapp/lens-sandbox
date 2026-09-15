#[cfg(not(target_os = "macos"))]
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(target_os = "macos"))]
use eframe::egui::ViewportId;

#[cfg(not(target_os = "macos"))]
use super::View;

static GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(not(target_os = "macos"))]
static WATCHING: AtomicBool = AtomicBool::new(false);
#[cfg(not(target_os = "macos"))]
static OPEN_REQUESTED: AtomicU8 = AtomicU8::new(NOTHING);
static CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<()>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(()).0);

pub fn subscribe() -> tokio::sync::watch::Receiver<()> {
    CHANGES.subscribe()
}

#[cfg(not(target_os = "macos"))]
const NOTHING: u8 = 0;
#[cfg(not(target_os = "macos"))]
const TIMELINE: u8 = 1;
#[cfg(not(target_os = "macos"))]
const APPROVALS: u8 = 2;

#[cfg(not(target_os = "macos"))]
pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("lns-audit-dashboard")
}

pub fn note_write() {
    GENERATION.fetch_add(1, Ordering::Release);
    CHANGES.send_replace(());
    #[cfg(not(target_os = "macos"))]
    if WATCHING.load(Ordering::Acquire)
        && let Some(ctx) = crate::approval_flow::window::ctx()
    {
        ctx.request_repaint_of(viewport_id());
    }
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[cfg(not(target_os = "macos"))]
pub fn set_watching(watching: bool) {
    WATCHING.store(watching, Ordering::Release);
}

/// Asks the tray thread to raise the window on this view; the newest request wins, because it is the one the user just made.
#[cfg(not(target_os = "macos"))]
pub fn request_open(view: View) {
    OPEN_REQUESTED.store(
        match view {
            View::Timeline => TIMELINE,
            View::Approvals => APPROVALS,
        },
        Ordering::Release,
    );
}

#[cfg(not(target_os = "macos"))]
pub fn take_open_request() -> Option<View> {
    match OPEN_REQUESTED.swap(NOTHING, Ordering::AcqRel) {
        TIMELINE => Some(View::Timeline),
        APPROVALS => Some(View::Approvals),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn external_clients_are_woken_when_dashboard_data_changes() {
        let mut receiver = subscribe();
        receiver.borrow_and_update();
        note_write();
        assert!(
            receiver.has_changed().unwrap(),
            "a native dashboard must learn about writes without polling files"
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn viewport_id_is_stable_and_not_the_root() {
        assert_eq!(viewport_id(), viewport_id());
        assert_ne!(viewport_id(), ViewportId::ROOT);
    }

    #[test]
    #[serial]
    fn note_write_advances_the_generation_without_a_watcher() {
        #[cfg(not(target_os = "macos"))]
        set_watching(false);
        let before = generation();
        note_write();
        assert!(generation() > before);
    }

    #[test]
    #[serial]
    #[cfg(not(target_os = "macos"))]
    fn note_write_wakes_the_installed_context_while_watching() {
        crate::approval_flow::window::install_ctx(eframe::egui::Context::default());
        set_watching(true);
        let before = generation();
        note_write();
        assert!(generation() > before);
        #[cfg(not(target_os = "macos"))]
        set_watching(false);
    }

    #[test]
    #[serial]
    #[cfg(not(target_os = "macos"))]
    fn the_open_request_is_a_one_shot_latch() {
        request_open(View::Timeline);
        assert_eq!(take_open_request(), Some(View::Timeline));
        assert_eq!(
            take_open_request(),
            None,
            "a window already raised must not raise itself again on the next frame"
        );
    }

    #[test]
    #[serial]
    #[cfg(not(target_os = "macos"))]
    fn the_menu_item_the_user_chose_is_the_view_the_window_opens_on() {
        request_open(View::Timeline);
        request_open(View::Approvals);
        assert_eq!(take_open_request(), Some(View::Approvals));
    }
}
