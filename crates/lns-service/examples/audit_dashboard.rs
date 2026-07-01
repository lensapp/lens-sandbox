use eframe::egui;
use lns_audit::TimelineRow;
use lns_service::approval_flow::window::{
    install_icon_font, install_system_fonts, quiet_debug_overlays,
};
use lns_service::dashboard::{self, DashboardAction, DashboardState, Sandbox};
use lns_service::tray::install_activation_policy;
use serde_json::{Value, json};

struct Preview {
    state: DashboardState,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = dashboard::background();
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {}

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if dashboard::render(ui, &mut self.state) == DashboardAction::Refresh {
            self.state = DashboardState::seeded(seed_rows(), seed_warnings(), seed_sandboxes());
        }
    }
}

fn row(ts: &str, run: &str, kind: &str, detail: &str, raw: Value) -> TimelineRow {
    TimelineRow {
        ts: ts.into(),
        when: ts.trim_end_matches('Z').replacen('T', " ", 1),
        run: run.into(),
        kind: kind.into(),
        detail: detail.into(),
        raw,
        integration: None,
    }
}

const RUN_A: &str = "9e8d7c6b0000000000000000000000aa";
const RUN_B: &str = "1a2b3c4d0000000000000000000000bb";

fn seed_rows() -> Vec<TimelineRow> {
    vec![
        row(
            "2026-06-29T15:12:00Z",
            RUN_A,
            "credential",
            "use some-provider fp 9c2f1a3d → api.some-provider.example",
            json!({"event":"credential_use","integration":"some-provider","fp":"9c2f1a3d","dest":["api.some-provider.example"]}),
        ),
        row(
            "2026-06-29T14:00:00Z",
            RUN_B,
            "connection",
            "connect some-oauth (oauth) @user [repo, read:org]",
            json!({"event":"connection","integration":"some-oauth","auth":"oauth","account":"@user","scopes":["repo","read:org"]}),
        ),
        row(
            "2026-06-29T13:32:00Z",
            RUN_B,
            "approval",
            "network allow-always api.example.test:443  [policy-ambiguous]",
            json!({"event":"approval","kind":"network","decision":"allow_always","target":"api.example.test:443","reason":"policy-ambiguous"}),
        ),
        row(
            "2026-06-29T13:30:00Z",
            RUN_B,
            "egress",
            "GET api.example.test:443 — allowed once → 200 success",
            json!({"action":"GET http://api.example.test:443/","result":"success","status_code":200}),
        ),
        row(
            "2026-06-29T13:05:00Z",
            RUN_B,
            "volume",
            "data → /data",
            json!({"type":"volume_attached","name":"data","target":"/data"}),
        ),
        row(
            "2026-06-29T13:00:01Z",
            RUN_B,
            "env",
            "injected: OPENAI_API_KEY, PATH",
            json!({"event":"run_env","env":{"OPENAI_API_KEY":"…","PATH":"…"}}),
        ),
    ]
}

fn seed_sandboxes() -> Vec<Sandbox> {
    vec![
        Sandbox {
            id: RUN_B.into(),
            name: "calm-finch".into(),
            status: "running".into(),
        },
        Sandbox {
            id: RUN_A.into(),
            name: "bold-otter".into(),
            status: "exited".into(),
        },
    ]
}

fn seed_warnings() -> Vec<String> {
    vec![]
}

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: dashboard::viewport_builder(),
        ..Default::default()
    };
    install_activation_policy(&mut options);
    eframe::run_native(
        "audit dashboard",
        options,
        Box::new(|cc| {
            quiet_debug_overlays(&cc.egui_ctx);
            install_system_fonts(&cc.egui_ctx);
            install_icon_font(&cc.egui_ctx);
            dashboard::style(&cc.egui_ctx);
            Ok(Box::new(Preview {
                state: DashboardState::seeded(seed_rows(), seed_warnings(), seed_sandboxes()),
            }))
        }),
    )
}
