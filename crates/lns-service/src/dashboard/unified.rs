use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin,
    RichText, Sense, Stroke, vec2,
};
use egui_material_icons::icons;

use super::state::{
    AuditColumns, CredentialColumns, CredentialReviewChoice, DashboardMode, DashboardView,
    RevealedKind, TABLE_CHEVRON_WIDTH, TABLE_RESIZER_WIDTH, TableColumnWidths,
    UnifiedDashboardAction, UnifiedDashboardState, authorization_detail, credential_access_label,
    credential_machine_label, credential_row_icon_color,
};
use super::{
    ACCENT_GREEN, BORDER, CATEGORY, CHROME_FILL, CONTENT_FILL, CredentialBinding,
    CredentialOperation, CredentialSummary, DETAIL_WIDTH, FS_BODY, FS_LABEL, FS_SECONDARY,
    INPUT_FILL, MODAL_FILL, SELECT_FILL, SIDEBAR_WIDTH, STATUS_CRITICAL, STATUS_WARNING,
    TEXT_MUTED, TEXT_PRIMARY,
    TRAFFIC_LIGHT_INSET, dashboard_banner, dashboard_section_label, detail_panel, glyph,
    icon_button, row_click, status_dot,
};
use crate::ui::button::{Button, ButtonKind};
use crate::ui::theme;

pub fn unified_viewport_builder() -> egui::ViewportBuilder {
    super::viewport_builder()
        .with_title("Lens Sandbox — Dashboard")
        .with_inner_size([1120.0, 720.0])
        .with_min_inner_size([760.0, 480.0])
}

pub fn render_unified_dashboard(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
) -> UnifiedDashboardAction {
    ui.ctx().set_visuals(super::dashboard_visuals());
    expire_revealed_value(ui.ctx(), state);
    if ui
        .ctx()
        .input(|input| input.modifiers.command && input.key_pressed(egui::Key::K))
    {
        state.audit.search_open = true;
    }
    unified_sidebar_toggle(ui, state);
    let action = unified_refresh_button(ui, state.mode);
    if state.sidebar_open {
        unified_sidebar(ui, state);
    }
    match state.view {
        DashboardView::Credentials if state.selected_connector.is_some() => {
            credential_detail_panel(ui, state);
        }
        DashboardView::Audit if state.audit.selected.is_some() => {
            audit_detail_panel(ui, state);
        }
        _ => {}
    }
    unified_central(ui, state);
    let search_reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("unified-dashboard-search-anim"),
        state.audit.search_open,
        0.12,
    );
    if state.audit.search_open || search_reveal > 0.002 {
        unified_search_modal(ui, state, search_reveal);
    }
    if state.confirmation.is_some() {
        confirmation_modal(ui, state);
    }
    if state.reviewing_request.is_some() {
        review_modal(ui, state);
    }
    if state.replacing_connector.is_some() {
        replace_modal(ui, state);
    }
    state
        .pending_command
        .take()
        .map_or(action, UnifiedDashboardAction::Command)
}

/// Repaints while something is revealed so the countdown expires on its own, without input.
fn expire_revealed_value(ctx: &egui::Context, state: &mut UnifiedDashboardState) {
    let now = ctx.input(|input| input.time);
    let focused = ctx.input(|input| input.viewport().focused != Some(false));
    if state.hide_expired_reveal(now, focused) {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn unified_refresh_button(ui: &mut egui::Ui, mode: DashboardMode) -> UnifiedDashboardAction {
    let clicked = egui::Area::new(egui::Id::new("unified-dashboard-refresh"))
        .anchor(Align2::RIGHT_TOP, vec2(-6.0, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icons::ICON_REFRESH)
                .on_hover_text(if mode == DashboardMode::Preview {
                    "Refresh preview"
                } else {
                    "Refresh"
                })
                .clicked()
        })
        .inner;
    if clicked {
        UnifiedDashboardAction::Refresh
    } else {
        UnifiedDashboardAction::None
    }
}

fn unified_sidebar_toggle(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let icon = if state.sidebar_open {
        icons::ICON_LEFT_PANEL_CLOSE
    } else {
        icons::ICON_LEFT_PANEL_OPEN
    };
    let clicked = egui::Area::new(egui::Id::new("unified-dashboard-sidebar-toggle"))
        .fixed_pos(egui::pos2(TRAFFIC_LIGHT_INSET, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icon)
                .on_hover_text("Toggle sandbox list")
                .clicked()
        })
        .inner;
    if clicked {
        state.sidebar_open = !state.sidebar_open;
    }
}

fn unified_sidebar(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let sandboxes = state.sandboxes.clone();
    egui::Panel::left("unified-dashboard-sidebar")
        .resizable(true)
        .default_size(SIDEBAR_WIDTH)
        .min_size(190.0)
        .max_size(340.0)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL).inner_margin(Margin::same(8)))
        .show_inside(ui, |ui| {
            ui.add_space(26.0);
            if search_sidebar_row(ui).clicked() {
                state.audit.search_open = true;
            }
            ui.add_space(2.0);
            if sidebar_row(
                ui,
                icons::ICON_DNS,
                "All sandboxes",
                "",
                state.selected_sandbox.is_none(),
                None,
                None,
            )
            .clicked()
            {
                state.select_sandbox(None);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("SANDBOXES").size(FS_LABEL).color(TEXT_MUTED));
            });
            ui.add_space(2.0);
            for sandbox in sandboxes {
                let pending = state.pending_count(&sandbox.id);
                let selected = state.selected_sandbox.as_deref() == Some(&sandbox.id);
                if sidebar_row(
                    ui,
                    icons::ICON_DNS,
                    &sandbox.name,
                    &sandbox.project,
                    selected,
                    Some(&sandbox.status),
                    (pending > 0).then_some(pending),
                )
                .clicked()
                {
                    state.select_sandbox(Some(sandbox.id));
                }
            }
        });
}

fn search_sidebar_row(ui: &mut egui::Ui) -> egui::Response {
    let response = Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph(ui, icons::ICON_SEARCH, TEXT_MUTED, 18.0);
                ui.add_space(7.0);
                ui.label(RichText::new("Search").size(FS_BODY).color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("⌘K").size(FS_LABEL).color(TEXT_MUTED));
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn sidebar_row(
    ui: &mut egui::Ui,
    icon: egui_material_icons::MaterialIcon,
    label: &str,
    detail: &str,
    selected: bool,
    status: Option<&str>,
    attention: Option<usize>,
) -> egui::Response {
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(status) = status {
                    status_dot(ui, status);
                } else {
                    glyph(ui, icon, TEXT_MUTED, 18.0);
                }
                ui.add_space(7.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(FS_BODY).color(TEXT_PRIMARY));
                        if let Some(count) = attention {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                compact_attention_badge(ui, count);
                            });
                        }
                    });
                    if !detail.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(detail).size(FS_LABEL).color(TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    }
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn unified_central(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(CONTENT_FILL)
                .inner_margin(Margin::same(theme::STACK_MARGIN)),
        )
        .show_inside(ui, |ui| {
            ui.add_space(12.0);
            view_tabs(ui, state);
            if let Some(error) = state.audit.last_error.as_deref() {
                ui.add_space(12.0);
                dashboard_banner(ui, error, STATUS_CRITICAL);
            }
            if let Some(notice) = state.notice.as_deref() {
                ui.add_space(12.0);
                inline_notice(ui, notice);
            }
            ui.add_space(18.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match state.view {
                    DashboardView::Credentials => credentials(ui, state),
                    DashboardView::Audit => audit(ui, state),
                });
        });
}

fn view_tabs(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        for view in [DashboardView::Credentials, DashboardView::Audit] {
            if view_tab(ui, view, state.view == view).clicked() {
                state.view = view;
                state.select_connector(None);
                state.audit.selected = None;
            }
        }
    });
}

fn view_tab(ui: &mut egui::Ui, view: DashboardView, selected: bool) -> egui::Response {
    let response = ui
        .add(
            egui::Button::new(
                RichText::new(view.label())
                    .size(FS_BODY)
                    .color(if selected { TEXT_PRIMARY } else { TEXT_MUTED }),
            )
            .frame(false)
            .sense(Sense::click()),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            Stroke::new(2.0, CATEGORY),
        );
    }
    response
}

fn inline_notice(ui: &mut egui::Ui, notice: &str) {
    ui.horizontal(|ui| {
        glyph(ui, icons::ICON_CHECK_CIRCLE, ACCENT_GREEN, 15.0);
        ui.label(RichText::new(notice).size(FS_SECONDARY).color(TEXT_MUTED));
    });
}

fn credentials(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let indices = state.visible_credentials();
    if indices.is_empty() {
        ui.colored_label(TEXT_MUTED, "No credentials match this sandbox.");
        return;
    }
    let table_width = ui.available_width() - 12.0;
    let columns = CredentialColumns::for_width(table_width, &state.table_columns);
    credential_table_header(ui, columns, &mut state.table_columns);
    ui.separator();
    let mut selected_connector = None;
    for index in indices {
        let credential = &state.credentials[index].summary;
        let selected = state.selected_connector.as_deref() == Some(&credential.connector_id);
        if credential_table_row(ui, state, credential, columns, selected).clicked() {
            selected_connector = Some(credential.connector_id.clone());
        }
        ui.separator();
    }
    if selected_connector.is_some() {
        state.select_connector(selected_connector);
    }
}

#[derive(Clone, Copy)]
struct Cell {
    width: f32,
    height: f32,
}

fn credential_table_header(
    ui: &mut egui::Ui,
    columns: CredentialColumns,
    stored: &mut TableColumnWidths,
) {
    Frame::new()
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                table_header_cell(ui, "Credential", columns.connector - TABLE_RESIZER_WIDTH);
                if let Some(width) =
                    table_column_resizer(ui, "shared-primary-column", columns.connector)
                {
                    stored.primary = Some(width);
                }
                if columns.wide {
                    table_header_cell(ui, "On this machine", columns.machine - TABLE_RESIZER_WIDTH);
                    if let Some(width) =
                        table_column_resizer(ui, "credential-machine-column", columns.machine)
                    {
                        stored.credential_machine = width;
                    }
                }
                table_header_cell(
                    ui,
                    "Sandbox access",
                    columns.access
                        - if columns.wide {
                            TABLE_RESIZER_WIDTH
                        } else {
                            0.0
                        },
                );
                if columns.wide {
                    if let Some(width) =
                        table_column_resizer(ui, "credential-access-column", columns.access)
                    {
                        stored.credential_access = width;
                    }
                    table_header_cell(ui, "Last activity", columns.activity);
                }
                exact_cell(
                    ui,
                    Cell {
                        width: TABLE_CHEVRON_WIDTH,
                        height: 18.0,
                    },
                    Layout::left_to_right(Align::Center),
                    |_| {},
                );
            });
        });
    ui.add_space(3.0);
}

fn table_header_cell(ui: &mut egui::Ui, label: &str, width: f32) {
    exact_cell(
        ui,
        Cell {
            width,
            height: 18.0,
        },
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
        },
    );
}

fn table_column_resizer(ui: &mut egui::Ui, id: &'static str, current_width: f32) -> Option<f32> {
    let (rect, response) = ui
        .push_id(id, |ui| {
            ui.allocate_exact_size(vec2(TABLE_RESIZER_WIDTH, 18.0), Sense::drag())
        })
        .inner;
    let response = response.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let color = if response.hovered() || response.dragged() {
        TEXT_MUTED
    } else {
        BORDER
    };
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, color));
    if response.dragged() {
        Some(current_width + response.drag_delta().x)
    } else {
        None
    }
}

fn exact_cell(
    ui: &mut egui::Ui,
    cell: Cell,
    layout: Layout,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let (rect, _) = ui.allocate_exact_size(vec2(cell.width, cell.height), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((rect.min.x.to_bits(), rect.min.y.to_bits()))
            .max_rect(rect)
            .layout(layout),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add_contents(&mut child);
}

const ROW_CELL_HEIGHT: f32 = 38.0;

fn row_cell(width: f32) -> Cell {
    Cell {
        width,
        height: ROW_CELL_HEIGHT,
    }
}

fn credential_table_row(
    ui: &mut egui::Ui,
    state: &UnifiedDashboardState,
    credential: &CredentialSummary,
    columns: CredentialColumns,
    selected: bool,
) -> egui::Response {
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    row_cell(columns.connector),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        glyph(
                            ui,
                            icons::ICON_KEY,
                            credential_row_icon_color(credential.status),
                            17.0,
                        );
                        ui.add_space(7.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&credential.display_name)
                                    .size(FS_BODY)
                                    .color(TEXT_PRIMARY),
                            );
                            ui.label(
                                RichText::new(&credential.connector_id)
                                    .size(FS_LABEL)
                                    .monospace()
                                    .color(TEXT_MUTED),
                            );
                        });
                    },
                );
                if columns.wide {
                    let (machine, machine_color) = credential_machine_label(credential);
                    exact_cell(
                        ui,
                        row_cell(columns.machine),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(machine)
                                    .size(FS_SECONDARY)
                                    .color(machine_color),
                            );
                        },
                    );
                }
                let (access, access_color) = credential_access_label(state, credential);
                exact_cell(
                    ui,
                    row_cell(columns.access),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(access).size(FS_SECONDARY).color(access_color));
                    },
                );
                if columns.wide {
                    exact_cell(
                        ui,
                        row_cell(columns.activity),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(
                                    credential.recent_activity.as_deref().unwrap_or("Never"),
                                )
                                .size(FS_SECONDARY)
                                .color(TEXT_MUTED),
                            );
                        },
                    );
                }
                exact_cell(
                    ui,
                    row_cell(TABLE_CHEVRON_WIDTH),
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| glyph(ui, icons::ICON_CHEVRON_RIGHT, TEXT_MUTED, 16.0),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn audit(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    for warning in &state.audit.warnings {
        dashboard_banner(ui, warning, STATUS_WARNING);
        ui.add_space(10.0);
    }
    let indices = state.visible_audit();
    if indices.is_empty() {
        ui.colored_label(TEXT_MUTED, "No audit events for this sandbox.");
        return;
    }
    let table_width = ui.available_width() - 12.0;
    let columns = AuditColumns::for_width(table_width, &state.table_columns);
    audit_table_header(ui, columns, &mut state.table_columns);
    ui.separator();
    for index in indices {
        audit_table_row(ui, state, index, columns);
        ui.separator();
    }
}

fn audit_table_header(ui: &mut egui::Ui, columns: AuditColumns, stored: &mut TableColumnWidths) {
    Frame::new()
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                table_header_cell(ui, "Event", columns.event - TABLE_RESIZER_WIDTH);
                if let Some(width) =
                    table_column_resizer(ui, "shared-primary-column", columns.event)
                {
                    stored.primary = Some(width);
                }
                if columns.wide {
                    table_header_cell(ui, "Sandbox", columns.sandbox - TABLE_RESIZER_WIDTH);
                    if let Some(width) =
                        table_column_resizer(ui, "audit-sandbox-column", columns.sandbox)
                    {
                        stored.audit_sandbox = width;
                    }
                }
                table_header_cell(ui, "When", columns.when);
                exact_cell(
                    ui,
                    Cell {
                        width: TABLE_CHEVRON_WIDTH,
                        height: 18.0,
                    },
                    Layout::left_to_right(Align::Center),
                    |_| {},
                );
            });
        });
    ui.add_space(3.0);
}

fn audit_table_row(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    index: usize,
    columns: AuditColumns,
) {
    let row = state.audit.rows[index].clone();
    let selected = state.audit.selected == Some(index);
    let sandbox = state.sandbox_label_for_run(&row.run);
    let when = super::format::friendly_time(super::now_unix_secs(), &row.ts);
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    row_cell(columns.event),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        glyph(
                            ui,
                            super::kind_icon(&row.kind),
                            super::kind_color(&row.kind),
                            16.0,
                        );
                        ui.add_space(7.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&row.detail).size(FS_BODY).color(TEXT_PRIMARY),
                            )
                            .truncate(),
                        );
                    },
                );
                if columns.wide {
                    exact_cell(
                        ui,
                        row_cell(columns.sandbox),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&sandbox).size(FS_SECONDARY).color(TEXT_MUTED),
                                )
                                .truncate(),
                            );
                        },
                    );
                }
                exact_cell(
                    ui,
                    row_cell(columns.when),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(&when).size(FS_SECONDARY).color(TEXT_MUTED));
                    },
                );
                exact_cell(
                    ui,
                    row_cell(TABLE_CHEVRON_WIDTH),
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| glyph(ui, icons::ICON_CHEVRON_RIGHT, TEXT_MUTED, 16.0),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    if response.clicked() {
        state.audit.selected = Some(index);
        state.audit.detail_row = Some(row);
    }
}

fn audit_detail_panel(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("unified-audit-detail-anim"),
        state.audit.selected.is_some(),
        0.16,
    );
    let credential_link = state
        .audit
        .detail_row
        .as_ref()
        .and_then(|row| row.connector.as_deref())
        .is_some_and(|connector| {
            state
                .credentials
                .iter()
                .any(|credential| credential.summary.connector_id == connector)
        });
    if let Some(connector) = detail_panel(ui, &mut state.audit, reveal, credential_link) {
        state.view = DashboardView::Credentials;
        state.audit.selected = None;
        state.audit.detail_row = None;
        state.select_connector(Some(connector));
    }
}

enum UnifiedSearchPick {
    Credential(String),
    Audit(usize),
}

fn unified_search_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState, reveal: f32) {
    let mut pick = None;
    let backdrop_alpha = (14.0 * reveal) as u8;
    let shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 2,
        color: Color32::from_black_alpha((160.0 * reveal) as u8),
    };
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-search"))
        .backdrop_color(Color32::from_rgba_premultiplied(0, 0, 0, backdrop_alpha))
        .frame(
            Frame::new()
                .fill(MODAL_FILL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12))
                .shadow(shadow)
                .inner_margin(Margin::same(14)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_opacity(reveal);
            ui.set_width(560.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.audit.search_query)
                    .hint_text("Search credentials and activity")
                    .background_color(MODAL_FILL)
                    .font(FontId::new(18.0, egui::FontFamily::Proportional))
                    .margin(Margin::symmetric(2, 8))
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            ui.add_space(10.0);
            let results = state.search_results(&state.audit.search_query);
            if results.credentials.is_empty() && results.audit.is_empty() {
                let message = if state.audit.search_query.trim().is_empty() {
                    "No credentials or activity."
                } else {
                    "No matching credentials or activity."
                };
                ui.colored_label(TEXT_MUTED, message);
            }
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !results.credentials.is_empty() {
                        search_group_label(ui, "Credentials");
                        for &index in &results.credentials {
                            let credential = &state.credentials[index].summary;
                            if search_row(
                                ui,
                                icons::ICON_KEY,
                                super::credential_status_color(credential.status),
                                &credential.display_name,
                                &credential.connector_id,
                                "Credential",
                            )
                            .clicked()
                            {
                                pick = Some(UnifiedSearchPick::Credential(
                                    credential.connector_id.clone(),
                                ));
                            }
                        }
                    }
                    if !results.audit.is_empty() {
                        if !results.credentials.is_empty() {
                            ui.add_space(8.0);
                        }
                        search_group_label(ui, "Audit");
                        for &index in &results.audit {
                            let row = &state.audit.rows[index];
                            if search_row(
                                ui,
                                super::kind_icon(&row.kind),
                                super::kind_color(&row.kind),
                                &row.detail,
                                &state.sandbox_label_for_run(&row.run),
                                "Audit",
                            )
                            .clicked()
                            {
                                pick = Some(UnifiedSearchPick::Audit(index));
                            }
                        }
                    }
                });
        });
    match pick {
        Some(UnifiedSearchPick::Credential(connector_id)) => {
            state.view = DashboardView::Credentials;
            state.audit.selected = None;
            state.audit.detail_row = None;
            state.audit.search_open = false;
            state.select_connector(Some(connector_id));
        }
        Some(UnifiedSearchPick::Audit(index)) => {
            state.view = DashboardView::Audit;
            state.select_connector(None);
            state.audit.selected = Some(index);
            state.audit.detail_row = Some(state.audit.rows[index].clone());
            state.audit.search_open = false;
        }
        None if state.audit.search_open && modal.should_close() => {
            state.audit.search_open = false;
        }
        None => {}
    }
}

fn search_group_label(ui: &mut egui::Ui, label: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(3.0);
}

fn search_row(
    ui: &mut egui::Ui,
    icon: egui_material_icons::MaterialIcon,
    color: Color32,
    title: &str,
    detail: &str,
    kind: &str,
) -> egui::Response {
    let response = Frame::new()
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph(ui, icon, color, 16.0);
                ui.add_space(7.0);
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(title).size(FS_BODY).color(TEXT_PRIMARY))
                            .truncate(),
                    );
                    ui.label(
                        RichText::new(detail)
                            .size(FS_LABEL)
                            .monospace()
                            .color(TEXT_MUTED),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(kind).size(FS_LABEL).color(TEXT_MUTED));
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn credential_detail_panel(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("unified-credential-detail-anim"),
        state.selected_connector.is_some(),
        0.16,
    );
    let Some(credential) = state.selected_credential().cloned() else {
        return;
    };
    egui::Panel::right("unified-credential-detail")
        .resizable(false)
        .exact_size(DETAIL_WIDTH * reveal)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL))
        .show_inside(ui, |ui| {
            ui.set_opacity(reveal);
            Frame::new()
                .inner_margin(Margin::same(theme::STACK_MARGIN))
                .show(ui, |ui| {
                    ui.set_width(DETAIL_WIDTH - 2.0 * theme::STACK_MARGIN as f32);
                    credential_detail(ui, state, &credential);
                });
        });
}

fn credential_detail(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    credential: &super::DashboardCredential,
) {
    ui.horizontal(|ui| {
        glyph(
            ui,
            icons::ICON_KEY,
            super::credential_status_color(credential.summary.status),
            18.0,
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(&credential.summary.display_name)
                    .size(FS_BODY)
                    .color(TEXT_PRIMARY),
            );
            ui.label(
                RichText::new(&credential.summary.connector_id)
                    .size(FS_LABEL)
                    .monospace()
                    .color(TEXT_MUTED),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, icons::ICON_CLOSE)
                .on_hover_text("Close credential details")
                .clicked()
            {
                state.select_connector(None);
            }
        });
    });
    ui.add_space(10.0);
    super::credential_status_badge(ui, credential.summary.status);
    ui.add_space(14.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(request) = credential.summary.pending.as_ref() {
                pending_detail(ui, state, request);
                ui.add_space(16.0);
            }
            dashboard_section_label(ui, "EFFECTIVE ACCESS");
            detail_field(ui, "Machine credential", credential.summary.binding.label());
            detail_field(
                ui,
                "Sandbox authorization",
                &authorization_detail(state, &credential.summary),
            );
            if let Some(env) = credential.summary.environment_variable.as_deref() {
                detail_field(ui, "Environment", env);
            }
            if let Some(account) = credential.summary.account.as_deref() {
                detail_field(ui, "Account", account);
            }
            if !credential.summary.scopes.is_empty() {
                detail_field(ui, "Scopes", &credential.summary.scopes.join(", "));
            }
            if let Some(value) = credential.value.as_deref() {
                sensitive_value(
                    ui,
                    state,
                    &credential.summary.connector_id,
                    "Real machine credential",
                    value,
                    RevealedKind::Value,
                );
            }
            if let Some(placeholder) = credential.placeholder.as_deref() {
                sensitive_value(
                    ui,
                    state,
                    &credential.summary.connector_id,
                    "Fake workload placeholder",
                    placeholder,
                    RevealedKind::Placeholder,
                );
            }
            if !credential.summary.sandboxes.is_empty() {
                ui.add_space(8.0);
                dashboard_section_label(ui, "SANDBOX ACCESS");
                let mut open_sandbox = None;
                for access in &credential.summary.sandboxes {
                    let navigable = state
                        .sandboxes
                        .iter()
                        .any(|sandbox| sandbox.id == access.sandbox_id);
                    if super::credential_access_row(ui, access, navigable).clicked() {
                        open_sandbox = Some(access.sandbox_id.clone());
                    }
                }
                if let Some(sandbox_id) = open_sandbox {
                    state.select_sandbox(Some(sandbox_id));
                }
            }
            if !credential.summary.destinations.is_empty() {
                ui.add_space(8.0);
                dashboard_section_label(ui, "DESTINATIONS");
                crate::ui::badge::badges(ui, &credential.summary.destinations);
                ui.add_space(10.0);
            }
            credential_actions(ui, state, credential);
        });
}

fn detail_field(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
    ui.add_space(9.0);
}

/// Masked until asked for, selectable only while revealed, and hidden again by [`UnifiedDashboardState::hide_expired_reveal`].
fn sensitive_value(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    connector_id: &str,
    label: &str,
    value: &str,
    kind: RevealedKind,
) {
    let revealed = state.is_revealed(connector_id, kind);
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    Frame::new()
        .fill(INPUT_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(if revealed { value } else { MASKED_VALUE })
                        .size(FS_SECONDARY)
                        .monospace()
                        .color(TEXT_PRIMARY),
                )
                .selectable(revealed),
            );
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::new("Copy", ButtonKind::Secondary)
                        .show(ui)
                        .clicked()
                    {
                        ui.ctx().copy_text(value.to_string());
                        state.notice = Some(copied_notice(label, state.mode));
                    }
                    if Button::new(
                        if revealed { "Hide" } else { "Reveal" },
                        ButtonKind::Secondary,
                    )
                    .show(ui)
                    .clicked()
                    {
                        state.toggle_reveal(connector_id, kind, ui.input(|input| input.time));
                    }
                });
            });
        });
    ui.add_space(9.0);
}

const MASKED_VALUE: &str = "••••••••••••••••";

/// Names the preview's data as synthetic, so a copy taken from it is never mistaken for a real credential.
fn copied_notice(label: &str, mode: DashboardMode) -> String {
    match mode {
        DashboardMode::Preview => format!("{label} copied from synthetic preview data."),
        DashboardMode::Live => format!("{label} copied."),
    }
}

fn pending_detail(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    request: &super::PendingCredentialRequest,
) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            STATUS_WARNING.r(),
            STATUS_WARNING.g(),
            STATUS_WARNING.b(),
            18,
        ))
        .stroke(Stroke::new(1.0, STATUS_WARNING))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Waiting for you")
                    .size(FS_BODY)
                    .color(STATUS_WARNING),
            );
            ui.label(
                RichText::new(format!(
                    "{} wants to {}",
                    request.sandbox_name, request.action
                ))
                .size(FS_SECONDARY)
                .color(TEXT_PRIMARY),
            );
            ui.add_space(10.0);
            if Button::new("Review", ButtonKind::Primary)
                .show(ui)
                .clicked()
            {
                state.reviewing_request = Some(request.id.clone());
            }
        });
}

fn credential_actions(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    credential: &super::DashboardCredential,
) {
    if credential.summary.pending.is_some() {
        return;
    }
    ui.add_space(6.0);
    dashboard_section_label(ui, "ACTIONS");
    let can_replace = credential.summary.binding == CredentialBinding::Stored;
    if can_replace
        && Button::new("Replace", ButtonKind::Secondary)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
    {
        state.replacing_connector = Some(credential.summary.connector_id.clone());
        state.replacement_value.clear();
    }
    let access = state
        .selected_sandbox
        .as_ref()
        .and_then(|id| {
            credential
                .summary
                .sandboxes
                .iter()
                .find(|access| &access.sandbox_id == id && access.active && access.revocable)
        })
        .or_else(|| {
            credential
                .summary
                .sandboxes
                .iter()
                .find(|access| access.active && access.revocable)
        });
    if let Some(access) = access {
        ui.add_space(6.0);
        if Button::new("Disconnect", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::DisconnectProject {
                connector_id: credential.summary.connector_id.clone(),
                sandbox_id: access.sandbox_id.clone(),
                project: access.project.clone(),
            });
        }
    }
    if credential.summary.binding != CredentialBinding::Unbound {
        ui.add_space(6.0);
        if Button::new("Remove credential", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::ForgetEverywhere {
                connector_id: credential.summary.connector_id.clone(),
            });
        }
    }
}

fn confirmation_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(operation) = state.confirmation.clone() else {
        return;
    };
    let mut confirmed = false;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-confirmation"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                RichText::new(operation.title())
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(operation.description())
                    .size(FS_BODY)
                    .color(TEXT_MUTED),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Button::new(operation.confirm_label(), ButtonKind::Danger)
                    .show(ui)
                    .clicked()
                {
                    confirmed = true;
                }
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.confirmation = None;
                }
            });
        });
    if confirmed {
        state.resolve_confirmation(operation);
    } else if modal.should_close() {
        state.confirmation = None;
    }
}

fn review_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(request_id) = state.reviewing_request.clone() else {
        return;
    };
    let Some((_, display_name, request)) = state.pending_request(&request_id) else {
        state.reviewing_request = None;
        return;
    };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-review"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(format!("Review {display_name}"))
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} in {} wants to {}.",
                    request.sandbox_name, request.project, request.action
                ))
                .size(FS_BODY)
                .color(TEXT_PRIMARY),
            );
            if let Some(code) = &request.user_code {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(code)
                        .size(18.0)
                        .monospace()
                        .color(TEXT_PRIMARY),
                );
            }
            if let Some(uri) = &request.verification_uri {
                ui.hyperlink_to("Open sign-in", uri);
            }
            ui.add_space(16.0);
            if !request.oauth || request.token_fallback {
                ui.add(
                    egui::TextEdit::singleline(&mut *state.review_value)
                        .password(true)
                        .hint_text("Credential value")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                decision = review_buttons(ui, &request, &state.review_value);
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.reviewing_request = None;
                    state.review_value.clear();
                }
            });
        });
    if let Some(choice) = decision {
        state.resolve_review(&request, choice);
    } else if modal.should_close() {
        state.reviewing_request = None;
        state.review_value.clear();
    }
}

/// Mirrors the approval card's choices, so answering here is never a worse decision than answering there: an already-bound value can be granted without re-entering it, and a fresh sign-in is labelled as the replacement it is.
fn review_buttons(
    ui: &mut egui::Ui,
    request: &super::PendingCredentialRequest,
    typed: &str,
) -> Option<CredentialReviewChoice> {
    let mut decision = None;
    if Button::new("Deny", ButtonKind::Danger).show(ui).clicked() {
        decision = Some(CredentialReviewChoice::Deny);
    }
    if !typed.trim().is_empty()
        && Button::new(
            if request.oauth {
                "Use token"
            } else {
                "Use value"
            },
            ButtonKind::Primary,
        )
        .show(ui)
        .clicked()
    {
        decision = Some(CredentialReviewChoice::UseValue(typed.to_string().into()));
    }
    if request.oauth {
        if request.verification_uri.is_none()
            && Button::new(
                if request.bound_value_available {
                    "Reconnect"
                } else {
                    "Connect"
                },
                ButtonKind::Primary,
            )
            .show(ui)
            .clicked()
        {
            decision = Some(CredentialReviewChoice::Connect);
        }
    } else if request.host_value_available
        && Button::new("Use host", ButtonKind::Primary)
            .show(ui)
            .clicked()
    {
        decision = Some(CredentialReviewChoice::UseHost);
    }
    if request.bound_value_available
        && Button::new(
            if request.oauth {
                "Use connection"
            } else {
                "Use connected value"
            },
            ButtonKind::Primary,
        )
        .show(ui)
        .clicked()
    {
        decision = Some(CredentialReviewChoice::UseBound);
    }
    decision
}

fn replace_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(connector_id) = state.replacing_connector.clone() else {
        return;
    };
    let mut save = false;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-replace"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                RichText::new("Replace stored value")
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Enter a new {}value for {connector_id}. The previous value will be replaced.",
                    if state.mode == DashboardMode::Preview {
                        "synthetic "
                    } else {
                        ""
                    }
                ))
                .size(FS_BODY)
                .color(TEXT_MUTED),
            );
            ui.add_space(12.0);
            ui.add(
                egui::TextEdit::singleline(&mut *state.replacement_value)
                    .password(true)
                    .hint_text(if state.mode == DashboardMode::Preview {
                        "Synthetic credential value"
                    } else {
                        "Credential value"
                    })
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!state.replacement_value.trim().is_empty(), |ui| {
                    if Button::new("Replace", ButtonKind::Primary)
                        .show(ui)
                        .clicked()
                    {
                        save = true;
                    }
                });
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.replacing_connector = None;
                    state.replacement_value.clear();
                }
            });
        });
    if save {
        state.resolve_replacement(&connector_id);
    } else if modal.should_close() {
        state.replacing_connector = None;
        state.replacement_value.clear();
    }
}

fn modal_frame() -> Frame {
    Frame::new()
        .fill(MODAL_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
}

fn compact_attention_badge(ui: &mut egui::Ui, count: usize) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            STATUS_WARNING.r(),
            STATUS_WARNING.g(),
            STATUS_WARNING.b(),
            24,
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(count.to_string())
                    .size(FS_LABEL)
                    .color(STATUS_WARNING),
            )
            .on_hover_text(format!(
                "{count} pending credential request{}",
                if count == 1 { "" } else { "s" }
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_table_divider_changes_its_width() {
        let ctx = egui::Context::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 100.0));
        let render = |events, resized: &mut Option<f32>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(screen_rect),
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        exact_cell(
                            ui,
                            Cell {
                                width: 90.0,
                                height: 18.0,
                            },
                            Layout::left_to_right(Align::Center),
                            |_| {},
                        );
                        *resized = table_column_resizer(ui, "test-column", 100.0);
                    });
                },
            );
        };
        let mut resized = None;
        render(Vec::new(), &mut resized);
        render(
            vec![
                egui::Event::PointerMoved(egui::pos2(95.0, 9.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(95.0, 9.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut resized,
        );
        render(
            vec![egui::Event::PointerMoved(egui::pos2(115.0, 9.0))],
            &mut resized,
        );

        assert_eq!(resized, Some(120.0));
    }
}
