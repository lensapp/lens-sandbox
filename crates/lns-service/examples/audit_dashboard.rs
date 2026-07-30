use eframe::egui;
use lns_audit::TimelineRow;
use lns_service::approval_flow::window::{
    install_icon_font, install_system_fonts, quiet_debug_overlays,
};
use lns_service::dashboard::{self, DashboardAction, DashboardState, Sandbox};
use lns_service::tray::install_activation_policy;
use serde_json::{Map, Value, json};

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

const RUN_A: &str = "9e8d7c6b0000000000000000000000aa";
const RUN_B: &str = "1a2b3c4d0000000000000000000000bb";

fn ctx<'a>(ts: &'a str, run: &'a str, microvm: &'a str) -> lns_ocsf::Context<'a> {
    lns_ocsf::Context {
        time_unix_secs: 1_782_972_000,
        ts_rfc3339: ts,
        run,
        microvm,
    }
}

fn ocsf_row(ts: &str, run: &str, connector: Option<&str>, raw: Value) -> TimelineRow {
    let kind = raw["unmapped"]["lns_kind"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let detail = raw["message"].as_str().unwrap_or_default().to_string();
    TimelineRow {
        ts: ts.into(),
        when: ts.trim_end_matches('Z').replacen('T', " ", 1),
        run: run.into(),
        kind,
        detail,
        raw,
        connector: connector.map(str::to_string),
    }
}

fn seed_egress() -> Value {
    let mut ev = lns_ocsf::egress(
        &ctx("2026-06-29T13:30:00Z", RUN_B, "calm-finch"),
        "GET",
        "http://api.example.test:443/",
        Some(200),
        Some("success"),
        Some("user-allowed-once"),
        true,
    );
    if let Some(obj) = ev.as_object_mut() {
        obj.insert(
            "src_endpoint".into(),
            json!({"ip": "10.0.0.5", "port": 41176}),
        );
        obj.insert(
            "actor".into(),
            json!({"process": {"name": "curl", "pid": 57}}),
        );
    }
    ev
}

fn seed_rows() -> Vec<TimelineRow> {
    let mut env = Map::new();
    env.insert("OPENAI_API_KEY".into(), "…".into());
    env.insert("PATH".into(), "…".into());
    vec![
        ocsf_row(
            "2026-06-29T13:00:00Z",
            RUN_B,
            None,
            lns_ocsf::workload_launch(
                &ctx("2026-06-29T13:00:00Z", RUN_B, "calm-finch"),
                "alpine:latest",
            ),
        ),
        ocsf_row(
            "2026-06-29T15:12:00Z",
            RUN_A,
            Some("some-provider"),
            lns_ocsf::credential_use(
                &ctx("2026-06-29T15:12:00Z", RUN_A, "bold-otter"),
                "some-provider",
                "apikey",
                Some("9c2f1a3d"),
                &["api.some-provider.example".into()],
            ),
        ),
        ocsf_row(
            "2026-06-29T14:00:00Z",
            RUN_B,
            Some("some-oauth"),
            lns_ocsf::connection(
                &ctx("2026-06-29T14:00:00Z", RUN_B, "calm-finch"),
                "some-oauth",
                "oauth",
                Some("@user"),
                &["repo".into(), "read:org".into()],
                Some("2026-07-29T00:00:00Z"),
            ),
        ),
        ocsf_row(
            "2026-06-29T13:32:00Z",
            RUN_B,
            None,
            lns_ocsf::approval(
                &ctx("2026-06-29T13:32:00Z", RUN_B, "calm-finch"),
                "network",
                "api.example.test:443",
                "allow_always",
                Some("policy-ambiguous"),
                None,
            ),
        ),
        ocsf_row("2026-06-29T13:30:00Z", RUN_B, None, seed_egress()),
        ocsf_row(
            "2026-06-29T13:05:00Z",
            RUN_B,
            None,
            lns_ocsf::volume_mount(
                &ctx("2026-06-29T13:05:00Z", RUN_B, "calm-finch"),
                "data",
                "/data",
            ),
        ),
        ocsf_row(
            "2026-06-29T13:00:01Z",
            RUN_B,
            None,
            lns_ocsf::run_env(&ctx("2026-06-29T13:00:01Z", RUN_B, "calm-finch"), &env),
        ),
    ]
}

fn seed_sandboxes() -> Vec<Sandbox> {
    vec![
        Sandbox {
            id: RUN_B.into(),
            name: "calm-finch".into(),
            image: "alpine:latest".into(),
            status: "running".into(),
        },
        Sandbox {
            id: RUN_A.into(),
            name: "bold-otter".into(),
            image: "ubuntu:24.04".into(),
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
            lns_service::ui::texture_delta_guard::install(&cc.egui_ctx);
            install_system_fonts(&cc.egui_ctx);
            install_icon_font(&cc.egui_ctx);
            dashboard::apply_theme(&cc.egui_ctx);
            Ok(Box::new(Preview {
                state: DashboardState::seeded(seed_rows(), seed_warnings(), seed_sandboxes()),
            }))
        }),
    )
}
