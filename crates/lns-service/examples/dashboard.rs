use eframe::egui;
use lns_audit::TimelineRow;
use lns_service::approval_flow::window::{
    install_icon_font, install_system_fonts, quiet_debug_overlays,
};
use lns_service::dashboard::{
    self, CredentialBinding, CredentialStatus, CredentialSummary, DashboardCredential,
    DashboardSandbox, PendingCredentialRequest, SandboxAccess, UnifiedDashboardAction,
    UnifiedDashboardState,
};
use lns_service::tray::install_activation_policy;
use serde_json::json;

const FORECAST_RUN: &str = "1a2b3c4d0000000000000000000000bb";
const RESEARCH_RUN: &str = "9e8d7c6b0000000000000000000000aa";
const RELEASE_RUN: &str = "7f6e5d4c0000000000000000000000cc";

struct Preview {
    state: UnifiedDashboardState,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let color = dashboard::background();
        [
            color.r() as f32 / 255.0,
            color.g() as f32 / 255.0,
            color.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {}

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match dashboard::render_unified_dashboard(ui, &mut self.state) {
            UnifiedDashboardAction::Refresh => {
                self.state = seed_state();
                self.state.notice = Some("Synthetic dashboard state refreshed.".into());
            }
            UnifiedDashboardAction::None | UnifiedDashboardAction::Command(_) => {}
        }
    }
}

fn sandbox(
    id: &str,
    name: &str,
    project: &str,
    image: &str,
    status: &str,
    run_ids: &[&str],
) -> DashboardSandbox {
    DashboardSandbox {
        id: id.into(),
        name: name.into(),
        project: project.into(),
        image: image.into(),
        status: status.into(),
        run_ids: run_ids.iter().map(|run| (*run).into()).collect(),
    }
}

fn seed_sandboxes() -> Vec<DashboardSandbox> {
    vec![
        sandbox(
            "forecast-agent",
            "forecast-agent",
            "~/projects/forecast-agent",
            "alpine:latest",
            "running",
            &[FORECAST_RUN],
        ),
        sandbox(
            "release-agent",
            "release-agent",
            "~/projects/release-agent",
            "debian:bookworm",
            "running",
            &[RELEASE_RUN],
        ),
        sandbox(
            "research-helper",
            "research-helper",
            "~/projects/research-helper",
            "ubuntu:24.04",
            "exited",
            &[RESEARCH_RUN],
        ),
    ]
}

fn access(
    sandbox_id: &str,
    sandbox_name: &str,
    project: &str,
    reason: &str,
    active: bool,
) -> SandboxAccess {
    SandboxAccess {
        sandbox_id: sandbox_id.into(),
        sandbox_name: sandbox_name.into(),
        project: project.into(),
        reason: reason.into(),
        active,
        revocable: true,
    }
}

struct CredentialDetails<'a> {
    destinations: &'a [&'a str],
    sandboxes: Vec<SandboxAccess>,
    value: Option<&'a str>,
}

fn credential(
    connector_id: &str,
    display_name: &str,
    binding: CredentialBinding,
    status: CredentialStatus,
    environment_variable: &str,
    details: CredentialDetails<'_>,
) -> DashboardCredential {
    DashboardCredential {
        summary: CredentialSummary {
            connector_id: connector_id.into(),
            display_name: display_name.into(),
            binding,
            status,
            account: None,
            scopes: Vec::new(),
            expires_at: None,
            environment_variable: Some(environment_variable.into()),
            destinations: details
                .destinations
                .iter()
                .map(|destination| (*destination).into())
                .collect(),
            sandboxes: details.sandboxes,
            recent_activity: None,
            pending: None,
        },
        value: details.value.map(|value| value.to_string().into()),
    }
}

fn seed_credentials() -> Vec<DashboardCredential> {
    let mut pending = credential(
        "github",
        "GitHub",
        CredentialBinding::Unbound,
        CredentialStatus::Pending,
        "GH_TOKEN",
        CredentialDetails {
            destinations: &["api.github.com", "github.com"],
            sandboxes: Vec::new(),
            value: Some("gho_DEMO00000000000000000000000000000001"),
        },
    );
    pending.summary.scopes = vec!["repo".into(), "read:org".into()];
    pending.summary.recent_activity = Some("Requested just now".into());
    pending.summary.pending = Some(PendingCredentialRequest {
        id: "request-01".into(),
        sandbox_id: "forecast-agent".into(),
        sandbox_name: "forecast-agent".into(),
        project: "~/projects/forecast-agent".into(),
        action: "connect to api.github.com".into(),
        host_value_available: false,
        bound_value_available: true,
        oauth: true,
        token_fallback: true,
        verification_uri: None,
        user_code: None,
    });

    let mut stored = credential(
        "openai",
        "OpenAI",
        CredentialBinding::Stored,
        CredentialStatus::Active,
        "OPENAI_API_KEY",
        CredentialDetails {
            destinations: &["api.openai.com"],
            sandboxes: vec![
                access(
                    "forecast-agent",
                    "forecast-agent",
                    "~/projects/forecast-agent",
                    "Connected by project policy",
                    true,
                ),
                access(
                    "research-helper",
                    "research-helper",
                    "~/projects/research-helper",
                    "Required by sandbox definition",
                    true,
                ),
            ],
            value: Some("sk-DEMO000000000000000000000000000000000000000001"),
        },
    );
    stored.summary.recent_activity = Some("Used 12 min ago".into());

    let mut sign_in = credential(
        "google",
        "Google",
        CredentialBinding::OAuth,
        CredentialStatus::Expiring,
        "GOOGLE_OAUTH_ACCESS_TOKEN",
        CredentialDetails {
            destinations: &["www.googleapis.com"],
            sandboxes: vec![access(
                "forecast-agent",
                "forecast-agent",
                "~/projects/forecast-agent",
                "Connected by project policy",
                true,
            )],
            value: Some("ya29.DEMO0000000000000000000000000000000"),
        },
    );
    sign_in.summary.account = Some("sample-dev@gmail.com".into());
    sign_in.summary.scopes = vec!["openid".into(), "email".into(), "profile".into()];
    sign_in.summary.expires_at = Some("Tomorrow at 09:00".into());
    sign_in.summary.recent_activity = Some("Used yesterday".into());

    let mut host = credential(
        "anthropic",
        "Anthropic",
        CredentialBinding::HostDetected,
        CredentialStatus::Active,
        "ANTHROPIC_API_KEY",
        CredentialDetails {
            destinations: &["api.anthropic.com"],
            sandboxes: vec![access(
                "release-agent",
                "release-agent",
                "~/projects/release-agent",
                "Granted by sandbox definition",
                true,
            )],
            value: Some("sk-ant-api03-DEMO000000000000000000000000-DEMO01"),
        },
    );
    host.summary.recent_activity = Some("Used 3 days ago".into());

    let mut denied = credential(
        "linear",
        "Linear",
        CredentialBinding::Denied,
        CredentialStatus::Denied,
        "LINEAR_API_KEY",
        CredentialDetails {
            destinations: &["api.linear.app"],
            sandboxes: vec![access(
                "research-helper",
                "research-helper",
                "~/projects/research-helper",
                "Requested by sandbox definition",
                false,
            )],
            value: None,
        },
    );
    denied.summary.recent_activity = Some("Denied 6 days ago".into());

    vec![pending, stored, sign_in, host, denied]
}

fn audit_row(
    ts: &str,
    when: &str,
    run: &str,
    kind: &str,
    detail: &str,
    connector: Option<&str>,
) -> TimelineRow {
    TimelineRow {
        ts: ts.into(),
        when: when.into(),
        run: run.into(),
        kind: kind.into(),
        detail: detail.into(),
        raw: json!({
            "message": detail,
            "run": run,
            "unmapped": {
                "lns_kind": kind,
                "preview": true
            }
        }),
        connector: connector.map(str::to_string),
    }
}

fn seed_rows() -> Vec<TimelineRow> {
    vec![
        audit_row(
            "2026-07-27T11:48:00Z",
            "2026-07-27 11:48:00",
            FORECAST_RUN,
            "credential",
            "Used OpenAI at api.openai.com",
            Some("openai"),
        ),
        audit_row(
            "2026-07-27T11:43:00Z",
            "2026-07-27 11:43:00",
            FORECAST_RUN,
            "approval",
            "Allowed GET pypi.org once",
            None,
        ),
        audit_row(
            "2026-07-27T11:41:00Z",
            "2026-07-27 11:41:00",
            FORECAST_RUN,
            "egress",
            "GET pypi.org:443 returned 200",
            None,
        ),
        audit_row(
            "2026-07-27T10:20:00Z",
            "2026-07-27 10:20:00",
            RELEASE_RUN,
            "credential",
            "Used Anthropic at api.anthropic.com",
            Some("anthropic"),
        ),
        audit_row(
            "2026-07-26T15:12:00Z",
            "2026-07-26 15:12:00",
            RESEARCH_RUN,
            "credential",
            "Denied Linear",
            Some("linear"),
        ),
        audit_row(
            "2026-07-26T15:00:00Z",
            "2026-07-26 15:00:00",
            RESEARCH_RUN,
            "launch",
            "Started ubuntu:24.04",
            None,
        ),
    ]
}

fn seed_state() -> UnifiedDashboardState {
    let mut state = UnifiedDashboardState::seeded(
        seed_sandboxes(),
        seed_credentials(),
        seed_rows(),
        Vec::new(),
    );
    if std::env::var_os("DASHBOARD_PREVIEW_OPEN_DETAIL").is_some() {
        state.open_connector("github".into());
    }
    state
}

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: dashboard::unified_viewport_builder(),
        ..Default::default()
    };
    install_activation_policy(&mut options);
    eframe::run_native(
        "dashboard",
        options,
        Box::new(|cc| {
            quiet_debug_overlays(&cc.egui_ctx);
            lns_service::ui::texture_delta_guard::install(&cc.egui_ctx);
            install_system_fonts(&cc.egui_ctx);
            install_icon_font(&cc.egui_ctx);
            dashboard::apply_theme(&cc.egui_ctx);
            Ok(Box::new(Preview {
                state: seed_state(),
            }))
        }),
    )
}
