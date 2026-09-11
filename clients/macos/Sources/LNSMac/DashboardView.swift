import AppKit
import SwiftUI
import LNSClient

@MainActor
struct DashboardView: View {
    @ObservedObject var model: DashboardModel
    @ObservedObject var live: AppModel

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 190, ideal: 230, max: 320)
        } detail: {
            VStack(spacing: 0) {
                notices
                switch model.page {
                case .sandboxes: SandboxList(model: model)
                case .connectors: ConnectorCards(model: model)
                case .registries: RegistryList(model: model)
                case .audit: AuditTimeline(model: model)
                case .approvals: ApprovalHistory(model: model)
                }
            }
            .frame(minWidth: 540, minHeight: 400)
            .navigationTitle(model.page.rawValue)
            .navigationSubtitle(model.sandboxName)
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
        .task { await model.watch() }
        .sheet(item: $model.managementSheet) { sheet in
            ManagementForm(model: model, sheet: sheet).id(sheet.id)
        }
        .sheet(isPresented: $model.creatingSandbox) { SandboxCreateForm(model: model) }
        .onChange(of: model.page) { _ in model.selectedEvent = nil; model.clearHistory() }
        .onAppear { NSApplication.shared.setActivationPolicy(.regular) }
    }

    private var sidebar: some View {
        List {
            Section {
                ForEach(DashboardPage.allCases) { page in
                    Button {
                        model.page = page
                    } label: {
                        HStack {
                            Label(page.rawValue, systemImage: page.symbol)
                            Spacer()
                            if page == .approvals, model.waitingCount > 0 {
                                Text("\(model.waitingCount)").font(.caption.monospacedDigit())
                            }
                            if model.page == page { Image(systemName: "checkmark").accessibilityLabel("Selected") }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityValue(model.page == page ? "Selected" : "")
                }
            }
            if model.page == .audit || model.page == .approvals {
                Section("Sandboxes") {
                    sandboxButton(id: nil, name: "All sandboxes", image: "", status: "")
                    ForEach(model.data.sandboxes) { sandbox in
                        sandboxButton(id: sandbox.id, name: sandbox.name, image: sandbox.image, status: sandbox.status)
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            Label(live.startingService ? "Starting service…" : model.connected ? "Connected to service" : "Service disconnected",
                  systemImage: model.connected ? "checkmark.circle" : "wifi.exclamationmark")
                .font(.caption).foregroundStyle(.secondary).padding(12)
        }
    }

    private func sandboxButton(id: String?, name: String, image: String, status: String) -> some View {
        Button { model.selectSandbox(id) } label: {
            HStack(alignment: .top) {
                Image(systemName: id == nil ? "square.stack.3d.up" : "shippingbox")
                    .foregroundStyle(status == "running" ? Color.green : Color.secondary)
                VStack(alignment: .leading, spacing: 3) {
                    Text(name).lineLimit(1)
                    if !image.isEmpty { Text(image).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                    if !status.isEmpty { Text(status).font(.caption2).foregroundStyle(.secondary) }
                }
                Spacer()
                if model.filters.sandbox == id { Image(systemName: "checkmark").accessibilityLabel("Selected") }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityValue(model.filters.sandbox == id ? "Selected" : "")
        .help(id ?? "Every sandbox's audit and approval history")
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
            .font(.callout).foregroundStyle(.orange)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10).textSelection(.enabled)
    }
}

@MainActor
struct AuditTimeline: View {
    @ObservedObject var model: DashboardModel
    @FocusState private var searchFocused: Bool
    private let kinds = ["launch", "egress", "env", "volume", "bind", "approval", "connection", "credential", "tool"]

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Button { searchFocused = true } label: { Label("Search audit", systemImage: "magnifyingglass") }
                    .labelStyle(.iconOnly).buttonStyle(.borderless).keyboardShortcut("f")
                    .help("Search all sandboxes (⌘F)")
                TextField("Search all sandboxes’ audit events", text: $model.filters.search)
                    .textFieldStyle(.roundedBorder).focused($searchFocused)
                    .accessibilityLabel("Search all sandboxes’ audit events")
                    .onExitCommand { model.filters.search = ""; searchFocused = false }
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
                Spacer()
                Text("\(model.events.count) events").font(.caption).foregroundStyle(.secondary)
            }
            .padding(12)
            Divider()
            HSplitView {
                Table(model.events, selection: $model.selectedEvent) {
                    TableColumn("When") { event in Text(event.when).monospacedDigit() }.width(min: 130, ideal: 155)
                    TableColumn("Kind", value: \.kind).width(min: 70, ideal: 90)
                    TableColumn("Event", value: \.detail)
                }
                .accessibilityLabel("Audit events")
                .overlay {
                    if model.events.isEmpty {
                        Text(model.connected ? "No matching audit events." : "Waiting for the service…")
                            .foregroundStyle(.secondary).allowsHitTesting(false)
                    }
                }
                .frame(minWidth: 420)
                if let event = model.detail {
                    AuditDetail(event: event, sandbox: model.data.sandboxes.first { $0.id == event.run }?.name) {
                        model.selectedEvent = nil
                    }
                    .frame(minWidth: 280, idealWidth: 360, maxWidth: 540)
                }
            }
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
            .padding(16)
        }
        .background(.background)
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
                Text(label).font(.caption).foregroundStyle(.secondary)
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
