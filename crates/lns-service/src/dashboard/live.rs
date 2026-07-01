use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use eframe::egui::ViewportId;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static WATCHING: AtomicBool = AtomicBool::new(false);
static OPEN_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("lns-audit-dashboard")
}

pub fn note_write() {
    GENERATION.fetch_add(1, Ordering::Release);
    if WATCHING.load(Ordering::Acquire)
        && let Some(ctx) = crate::approval_flow::window::ctx()
    {
        ctx.request_repaint_of(viewport_id());
    }
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn set_watching(watching: bool) {
    WATCHING.store(watching, Ordering::Release);
}

pub fn request_open() {
    OPEN_REQUESTED.store(true, Ordering::Release);
}

pub fn take_open_request() -> bool {
    OPEN_REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn viewport_id_is_stable_and_not_the_root() {
        assert_eq!(viewport_id(), viewport_id());
        assert_ne!(viewport_id(), ViewportId::ROOT);
    }

    #[test]
    #[serial]
    fn note_write_advances_the_generation_without_a_watcher() {
        set_watching(false);
        let before = generation();
        note_write();
        assert!(generation() > before);
    }

    #[test]
    #[serial]
    fn note_write_wakes_the_installed_context_while_watching() {
        crate::approval_flow::window::install_ctx(eframe::egui::Context::default());
        set_watching(true);
        let before = generation();
        note_write();
        assert!(generation() > before);
        set_watching(false);
    }

    #[test]
    #[serial]
    fn the_open_request_is_a_one_shot_latch() {
        request_open();
        assert!(take_open_request());
        assert!(!take_open_request());
    }
}
