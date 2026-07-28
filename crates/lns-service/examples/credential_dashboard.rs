use eframe::egui;
use lns_service::approval_flow::window::{
    install_icon_font, install_system_fonts, quiet_debug_overlays,
};
use lns_service::dashboard::{
    self, CredentialBinding, CredentialDashboardAction, CredentialDashboardState,
    CredentialSandbox, CredentialStatus, CredentialSummary, PendingCredentialRequest,
    SandboxAccess,
};
use lns_service::tray::install_activation_policy;

struct Preview {
    state: CredentialDashboardState,
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
        match dashboard::render_credentials(ui, &mut self.state) {
            CredentialDashboardAction::None => {}
            CredentialDashboardAction::Refresh => {
                self.state = seed_state();
                self.state.notice = Some("Synthetic credential access refreshed.".into());
            }
            CredentialDashboardAction::ReviewRequest(request_id) => {
                self.state.notice = Some(format!("Preview only: review request {request_id}."));
            }
            CredentialDashboardAction::Confirm(operation) => {
                self.state.notice = Some(format!("Preview only: {}", operation.description()));
            }
        }
    }
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

fn seed_credentials() -> Vec<CredentialSummary> {
    vec![
        CredentialSummary {
            connector_id: "some-oauth".into(),
            display_name: "Some OAuth".into(),
            binding: CredentialBinding::Unbound,
            status: CredentialStatus::Pending,
            account: None,
            scopes: vec!["read:items".into(), "write:items".into()],
            expires_at: None,
            environment_variable: Some("SOME_OAUTH_TOKEN".into()),
            destinations: vec!["api.some-oauth.example".into()],
            sandboxes: vec![],
            recent_activity: Some("Requested just now".into()),
            pending: Some(PendingCredentialRequest {
                id: "request-01".into(),
                sandbox_id: "run-calm-finch".into(),
                sandbox_name: "calm-finch".into(),
                project: "~/projects/forecast-agent".into(),
                action: "connect to api.some-oauth.example".into(),
                requested_at: "just now".into(),
                held_requests: 2,
                host_value_available: false,
                oauth: true,
                token_fallback: true,
                verification_uri: None,
                user_code: None,
            }),
        },
        CredentialSummary {
            connector_id: "some-provider".into(),
            display_name: "Some Provider".into(),
            binding: CredentialBinding::Stored,
            status: CredentialStatus::Active,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: Some("SOME_TOKEN".into()),
            destinations: vec![
                "api.some-provider.example".into(),
                "uploads.some-provider.example".into(),
            ],
            sandboxes: vec![
                access(
                    "run-calm-finch",
                    "calm-finch",
                    "~/projects/forecast-agent",
                    "Connected by project policy",
                    true,
                ),
                access(
                    "run-bold-otter",
                    "bold-otter",
                    "~/projects/research-helper",
                    "Granted to workload definition",
                    false,
                ),
            ],
            recent_activity: Some("Recorded 12 min ago".into()),
            pending: None,
        },
        CredentialSummary {
            connector_id: "some-sign-in".into(),
            display_name: "Some Sign-in".into(),
            binding: CredentialBinding::OAuth,
            status: CredentialStatus::Expiring,
            account: Some("@sample-user".into()),
            scopes: vec!["read:projects".into(), "read:profile".into()],
            expires_at: Some("Tomorrow at 09:00".into()),
            environment_variable: Some("SOME_SIGN_IN_TOKEN".into()),
            destinations: vec!["api.some-sign-in.example".into()],
            sandboxes: vec![access(
                "run-calm-finch",
                "calm-finch",
                "~/projects/forecast-agent",
                "Connected by project policy",
                true,
            )],
            recent_activity: Some("Recorded yesterday".into()),
            pending: None,
        },
        CredentialSummary {
            connector_id: "some-host-tool".into(),
            display_name: "Some Host Tool".into(),
            binding: CredentialBinding::HostDetected,
            status: CredentialStatus::Active,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: Some("SOME_HOST_TOKEN".into()),
            destinations: vec!["api.some-host-tool.example".into()],
            sandboxes: vec![access(
                "run-silver-tern",
                "silver-tern",
                "~/projects/release-agent",
                "Granted to workload definition",
                true,
            )],
            recent_activity: Some("Recorded 3 days ago".into()),
            pending: None,
        },
        CredentialSummary {
            connector_id: "some-blocked-provider".into(),
            display_name: "Some Blocked Provider".into(),
            binding: CredentialBinding::Denied,
            status: CredentialStatus::Denied,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: Some("SOME_BLOCKED_TOKEN".into()),
            destinations: vec!["api.some-blocked-provider.example".into()],
            sandboxes: vec![access(
                "run-bold-otter",
                "bold-otter",
                "~/projects/research-helper",
                "Requested by workload definition",
                false,
            )],
            recent_activity: Some("Denied 6 days ago".into()),
            pending: None,
        },
        CredentialSummary {
            connector_id: "some-expired-oauth".into(),
            display_name: "Some Expired OAuth".into(),
            binding: CredentialBinding::OAuth,
            status: CredentialStatus::Expired,
            account: Some("@former-user".into()),
            scopes: vec!["read:items".into()],
            expires_at: Some("Expired 2 days ago".into()),
            environment_variable: Some("SOME_EXPIRED_TOKEN".into()),
            destinations: vec!["api.some-expired-oauth.example".into()],
            sandboxes: vec![],
            recent_activity: Some("Expired 2 days ago".into()),
            pending: None,
        },
        CredentialSummary {
            connector_id: "some-missing-provider".into(),
            display_name: "Some Missing Provider".into(),
            binding: CredentialBinding::Unbound,
            status: CredentialStatus::Unavailable,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: Some("SOME_MISSING_TOKEN".into()),
            destinations: vec!["api.some-missing-provider.example".into()],
            sandboxes: vec![],
            recent_activity: None,
            pending: None,
        },
    ]
}

fn seed_sandboxes() -> Vec<CredentialSandbox> {
    vec![
        CredentialSandbox {
            id: "run-calm-finch".into(),
            name: "calm-finch".into(),
            project: "~/projects/forecast-agent".into(),
            status: "running".into(),
        },
        CredentialSandbox {
            id: "run-silver-tern".into(),
            name: "silver-tern".into(),
            project: "~/projects/release-agent".into(),
            status: "running".into(),
        },
        CredentialSandbox {
            id: "run-bold-otter".into(),
            name: "bold-otter".into(),
            project: "~/projects/research-helper".into(),
            status: "exited".into(),
        },
    ]
}

fn seed_state() -> CredentialDashboardState {
    CredentialDashboardState::seeded(seed_credentials(), seed_sandboxes())
}

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: dashboard::credential_viewport_builder(),
        ..Default::default()
    };
    install_activation_policy(&mut options);
    eframe::run_native(
        "credential dashboard",
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
