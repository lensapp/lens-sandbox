import AppKit
import SwiftUI
import LNSClient

@MainActor
struct DashboardView: View {
    @ObservedObject var model: DashboardModel
    @ObservedObject var live: AppModel
    var mark: NSImage?
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 210, ideal: 240, max: 320)
        } detail: {
            VStack(spacing: 0) {
                notices
                switch model.page {
                case .sandboxes: SandboxList(model: model)
                case .connectors: ConnectorCards(model: model)
                case .registries: RegistryList(model: model)
                case .audit: AuditTimeline(model: model)
                case .approvals: ApprovalHistory(model: model, live: live)
                }
            }
            .frame(minWidth: 540, minHeight: 400)
            .background(LNSTheme.canvas)
            .navigationTitle("LNS")
            .toolbar {
                ToolbarItem {
                    Button {
                        Task { await model.refresh() }
                    } label: { Label("Refresh", systemImage: "arrow.clockwise") }
                    .keyboardShortcut("r", modifiers: .command)
                    .disabled(model.loading)
                }
                ToolbarItem {
                    if model.loading { ProgressView().controlSize(.small) }
                }
            }
        }
        .lnsAppearance()
        .task { await model.watch() }
        .sheet(item: $model.managementSheet) { sheet in
            ManagementForm(model: model, sheet: sheet).id(sheet.id)
        }
        .sheet(isPresented: $model.creatingSandbox) { SandboxCreateForm(model: model) }
        .onChange(of: model.page) { page in
            model.selectedEvent = nil; model.clearHistory()
            if page != .approvals { live.reviewingApprovalID = nil }
        }
        .onAppear {
            NSApplication.shared.setActivationPolicy(.regular)
            let open = openWindow
            live.openDashboard = {
                open(id: "dashboard")
                NSApplication.shared.activate(ignoringOtherApps: true)
            }
        }
    }

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                if let mark {
                    Image(nsImage: mark).resizable().renderingMode(.template)
                        .scaledToFit().frame(width: 24, height: 24).accessibilityHidden(true)
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text("LNS").font(.system(size: 16, weight: .semibold))
                }
            }
            .foregroundStyle(LNSTheme.heading).padding(.horizontal, 24).padding(.vertical, 20)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    navigationGroup("WORKSPACE", pages: [.sandboxes, .connectors, .registries])
                    navigationGroup("ACTIVITY", pages: [.audit, .approvals])
                }
                .padding(.horizontal, 12).padding(.vertical, 24)
            }
            Divider()
            HStack(spacing: 8) {
                Circle().fill(model.connected ? LNSTheme.success : LNSTheme.warning).frame(width: 6, height: 6)
                Text(live.startingService ? "Starting service…" : model.connected ? "Service connected" : "Service disconnected")
                    .font(.caption).foregroundStyle(LNSTheme.muted)
            }.padding(20)
        }
        .background(LNSTheme.surface)
    }

    private func navigationGroup(_ title: String, pages: [DashboardPage]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            sectionLabel(title)
            ForEach(pages) { page in
                Button { model.page = page } label: {
                    HStack(spacing: 10) {
                        Image(systemName: page.symbol).frame(width: 18)
                            .foregroundStyle(model.page == page ? LNSTheme.accent : LNSTheme.muted)
                        Text(page.rawValue)
                        Spacer()
                        if page == .approvals, model.waitingCount > 0 {
                            Text("\(model.waitingCount)").font(.caption.monospacedDigit())
                                .padding(.horizontal, 6).padding(.vertical, 2)
                                .background(LNSTheme.accent.opacity(0.2), in: RoundedRectangle(cornerRadius: 4))
                        }
                    }
                }
                .buttonStyle(LNSNavigationStyle(selected: model.page == page))
                .accessibilityValue(model.page == page ? "Selected" : "")
            }
        }
    }

    private func sectionLabel(_ title: String) -> some View {
        Text(title).font(.system(size: 10, weight: .semibold)).tracking(1)
            .foregroundStyle(LNSTheme.muted).padding(.horizontal, 12).padding(.bottom, 6)
    }

    @ViewBuilder private var notices: some View {
        if let message = model.connectionNotice { notice(message) }
        if let message = model.notice { notice(message) }
        if model.page == .sandboxes || model.page == .connectors {
            if let message = model.management.error { notice(message) }
            if let message = model.management.message {
                Text(message).font(.callout).frame(maxWidth: .infinity, alignment: .leading).padding(10)
            }
        }
        if let message = live.notice { notice(message) }
        if !live.connected {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(live.startingService ? "Starting the background service…" : "The interface is waiting for the background service.")
                    Spacer()
                    Button("Start Service", action: live.startService).disabled(!live.canStartService)
                }
                Text("Socket: \(live.socketPath)").font(.caption).textSelection(.enabled)
                if !live.canStartService && !live.startingService {
                    Text("This interface-only build needs a separately started service. Use the matching build of lns service start.").font(.caption)
                }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(12)
        }
        ForEach(Array(model.data.warnings.enumerated()), id: \.offset) { _, message in notice(message) }
        ForEach(Array(live.snapshot.notices.enumerated()), id: \.offset) { _, message in notice(message) }
        if !live.snapshot.notices.isEmpty {
            Button("Clear notices", action: live.dismissNotices).disabled(!live.connected).padding(.bottom, 8)
        }
    }

    private func notice(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.triangle")
            .font(.callout).foregroundStyle(LNSTheme.warning)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12).background(LNSTheme.warning.opacity(0.08)).textSelection(.enabled)
    }
}

@MainActor
struct AuditTimeline: View {
    @ObservedObject var model: DashboardModel
    private let kinds = ["launch", "egress", "env", "volume", "bind", "approval", "connection", "credential", "tool"]

    var body: some View {
        VStack(spacing: 0) {
            LNSPageHeader(title: "Audit", subtitle: "Review sandbox activity and network decisions.") { EmptyView() }
            LNSSearchField(prompt: "Search all sandboxes’ audit events", text: $model.filters.search)
                .padding(.horizontal, 24).padding(.bottom, 12)
            HStack(spacing: 12) {
                HStack(spacing: 8) {
                    SandboxFilter(model: model)
                        .disabled(!model.filters.search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    Menu {
                        Button("All event kinds") { model.filters.kinds = [] }
                        Divider()
                        ForEach(kinds, id: \.self) { kind in
                            Toggle(kind.capitalized, isOn: Binding(
                                get: { model.filters.kinds.contains(kind) },
                                set: { on in if on { model.filters.kinds.insert(kind) } else { model.filters.kinds.remove(kind) } }
                            ))
                        }
                    } label: { Label(model.filters.kinds.isEmpty ? "All event kinds" : "\(model.filters.kinds.count) kinds", systemImage: "line.3.horizontal.decrease.circle") }
                    .disabled(!model.filters.search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                .fixedSize(horizontal: true, vertical: false).controlSize(.large)
                Spacer()
                Text("\(model.events.count) events").font(.caption).foregroundStyle(LNSTheme.muted)
            }
            .padding(.horizontal, 24).padding(.bottom, 16)
            HSplitView {
                Table(model.events, selection: $model.selectedEvent) {
                    TableColumn("When") { event in Text(event.when).monospacedDigit() }.width(min: 130, ideal: 155)
                    TableColumn("Kind", value: \.kind).width(min: 70, ideal: 90)
                    TableColumn("Event", value: \.detail)
                }
                .accessibilityLabel("Audit events")
                .scrollContentBackground(.hidden)
                .overlay {
                    if model.events.isEmpty {
                        LNSEmptyState(
                            symbol: "list.bullet.rectangle",
                            title: model.connected ? "No matching events" : "Waiting for the service…",
                            message: "Sandbox activity appears here. Try another search or filter."
                        ).allowsHitTesting(false)
                    }
                }
                .frame(minWidth: 280)
                if let event = model.detail {
                    AuditDetail(event: event, sandbox: model.data.sandboxes.first { $0.id == event.run }?.name) {
                        model.selectedEvent = nil
                    }
                    .frame(minWidth: 280, idealWidth: 360, maxWidth: 540)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 4)).lnsPanel()
            .padding(.horizontal, 24).padding(.bottom, 24)
        }
        .onChange(of: model.filters.search) { _ in model.selectedEvent = nil }
        .onChange(of: model.filters.kinds) { _ in model.selectedEvent = nil }
    }
}

@MainActor
struct AuditDetail: View {
    let event: DashboardEvent
    let sandbox: String?
    let close: () -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text(event.kind.capitalized).font(.headline)
                    Spacer()
                    Button(action: close) { Label("Close details", systemImage: "xmark") }.labelStyle(.iconOnly)
                }
                detail("When", event.ts)
                detail("Sandbox", sandbox ?? event.run)
                if sandbox != nil { detail("Run ID", event.run) }
                detail("Event", event.detail)
                ForEach(fields, id: \.key) { field in detail(field.key, field.value) }
                Divider()
                detail("Raw event", event.raw, monospaced: true)
            }
            .padding(24)
        }
        .background(LNSTheme.surface)
        .onExitCommand(perform: close)
    }

    private var fields: [(key: String, value: String)] {
        guard let object = try? JSONSerialization.jsonObject(with: Data(event.raw.utf8)) as? [String: Any] else { return [] }
        let omitted: Set<String> = ["prev_hash", "ts", "type", "event", "run", "microvm", "time", "cloud", "metadata", "unmapped", "activity_id", "category_uid", "class_uid", "type_uid", "severity_id", "status_id", "disposition_id"]
        return object.keys.sorted().compactMap { key in
            guard !omitted.contains(key), let value = object[key], !(value is NSNull) else { return nil }
            if let text = value as? String { return text.isEmpty ? nil : (key.replacingOccurrences(of: "_", with: " ").capitalized, text) }
            guard let bytes = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]),
                  let text = String(data: bytes, encoding: .utf8) else { return nil }
            return (key.replacingOccurrences(of: "_", with: " ").capitalized, text)
        }
    }

    private func detail(_ label: String, _ value: String, monospaced: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(label).font(.caption).foregroundStyle(LNSTheme.muted)
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(value, forType: .string)
                } label: { Label("Copy \(label)", systemImage: "doc.on.doc") }
                .labelStyle(.iconOnly).buttonStyle(.borderless)
            }
            Text(value).font(monospaced ? .system(.caption, design: .monospaced) : .body)
                .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
