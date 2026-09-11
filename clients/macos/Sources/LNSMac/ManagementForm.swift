import AppKit
import SwiftUI
import LNSClient

struct ManagementSheet: Identifiable {
    enum Kind: String {
        case install = "Install Connector", connect = "Connect Account", grant = "Grant Access"
        case forget = "Forget Decision", disconnect = "Disconnect", uninstall = "Uninstall Connector", remove = "Remove Sandbox"
    }
    let id = UUID()
    let kind: Kind
    var offer: ConnectorOffer?
    var sandbox: DashboardSandbox?
    var connection = ""
    var run = ""
    var returnToGrant = false
}

@MainActor
struct ManagementForm: View {
    @ObservedObject var model: DashboardModel
    let sheet: ManagementSheet
    @State private var selection = GrantSelection()
    @State private var source = ""
    @State private var label = ""
    @State private var values: [String: String] = [:]
    @State private var access = ""

    private var offer: ConnectorOffer? { sheet.offer }
    private var method: ConnectorMethod? { offer?.methods.first { $0.name == selection.method } }
    private var connections: [ConnectorConnection] { offer?.connections.filter { $0.method == selection.method } ?? [] }
    private var busy: Bool { model.management.busy }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 6) {
                Text(sheet.kind.rawValue).font(.system(size: 22, weight: .semibold)).foregroundStyle(LNSTheme.heading)
                if let offer { Text(offer.name).font(.headline).foregroundStyle(LNSTheme.muted) }
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    fields.disabled(busy)
                    if let error = model.management.error {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            HStack {
                if busy { ProgressView().controlSize(.small); Text("Waiting for the service…").font(.caption) }
                Spacer()
                Button("Cancel") { model.managementSheet = nil }.keyboardShortcut(.cancelAction).disabled(busy)
                Button(sheet.kind.rawValue, role: destructive ? .destructive : nil) { submit() }
                    .buttonStyle(.borderedProminent)
                    .disabled(command == nil || busy || !model.connected || !model.management.connected)
            }
        }
        .padding(24)
        .frame(width: 540, height: sheet.kind == .grant || sheet.kind == .connect ? 590 : 330)
        .lnsAppearance()
        .interactiveDismissDisabled(busy)
        .onAppear { selection.run = sheet.run }
        .onChange(of: selection.method) { _ in
            if sheet.kind == .connect { label = ""; values = [:] }
        }
        .onChange(of: access) { id in if let offer { selection.choose(id, in: offer) } }
        .onDisappear { values = [:] }
    }

    @ViewBuilder private var fields: some View {
        switch sheet.kind {
        case .install:
            Text("Install a published connector or a local connector document. Installing makes it available on this Mac; each sandbox still needs a grant.")
            TextField("Registry reference or absolute file path", text: $source).textFieldStyle(.roundedBorder)
            Button("Choose File…", action: chooseFile)
        case .connect:
            Text("Save a connection on this Mac. You choose which sandboxes may use it when you grant access.")
            methodPicker(connecting: true)
            if let method, method.offerable, method.auth_label != nil {
                if let help = method.help { Text(help).textSelection(.enabled) }
                TextField("Connection name, e.g. work", text: $label).textFieldStyle(.roundedBorder)
                if offer?.connections.contains(where: { $0.label == label.trimmingCharacters(in: .whitespacesAndNewlines) }) == true {
                    Text("That name is already used. Choose a new name to keep the existing account.").foregroundStyle(LNSTheme.warning)
                }
                ForEach(method.asks, id: \.self) { field in
                    SecureField(field, text: Binding(get: { values[field] ?? "" }, set: { values[field] = $0 }))
                        .textFieldStyle(.roundedBorder)
                }
                Text("Real credentials stay outside the workload.").font(.caption).foregroundStyle(LNSTheme.muted)
            }
        case .grant:
            sandboxPicker
            Picker("Access", selection: $access) {
                Text("Choose a connection or direct access").tag("")
                ForEach(offer?.grantOptions ?? []) { option in Text(option.label).tag(option.id) }
            }
            if offer?.methods.contains(where: { $0.offerable && $0.auth_label != nil }) == true {
                Button("Add Connection…") {
                    model.managementSheet = ManagementSheet(kind: .connect, offer: offer, run: selection.run, returnToGrant: true)
                }.disabled(busy)
            }
            if offer?.grantOptions.isEmpty == true {
                Text("Connect an account first, then choose it here to grant access.").font(.callout).foregroundStyle(LNSTheme.muted)
            }
            if let method, method.offerable {
                if method.auth_label != nil {
                    if let connection = connections.first(where: { $0.label == selection.connection }) {
                        DisclosureLine(title: "Connection authority", entries: connection.authority)
                    }
                }
                Divider()
                Text("This sandbox will receive").font(.headline)
                DisclosureLine(title: "Opens", entries: method.opens)
                DisclosureLine(title: "Writes", entries: method.writes)
                DisclosureLine(title: "Sets", entries: method.env + method.credentials)
                if let overrides = method.overrides {
                    DisclosureLine(title: "Overrides deny rules", entries: overrides)
                } else {
                    Text("Deny-rule overrides could not be checked for this sandbox.").font(.callout).foregroundStyle(LNSTheme.warning)
                }
                Text("A sandbox holds one grant per connector. Granting another connection or access option replaces its previous grant.")
                    .font(.caption).foregroundStyle(LNSTheme.muted)
            }
        case .forget:
            sandboxPicker
            Text("Clear this sandbox’s granted or declined decision about the connector. Its next start will ask again. Saved connections stay on this Mac.")
        case .disconnect:
            Text("Disconnect \(sheet.connection)?").font(.headline)
            Text("This removes the saved connection from this Mac. Existing sandbox grants remain; the next request that needs a credential will ask you to connect again.")
        case .uninstall:
            Text("Remove this connector and all of its saved connections from this Mac?")
            Text("Existing sandbox grants remain. Reinstalling the same connector bytes resumes those decisions.").foregroundStyle(LNSTheme.muted)
        case .remove:
            Text("Remove \(sheet.sandbox?.name ?? "this sandbox")?").font(.headline)
            Text("This deletes its writable layer, saved state, and decisions. Stop a running sandbox before removing it.")
        }
    }

    private var sandboxPicker: some View {
        Picker("Sandbox", selection: $selection.run) {
            Text("Choose a sandbox").tag("")
            ForEach(model.currentSandboxes) { sandbox in
                Text("\(sandbox.name) · \(sandbox.statusLabel)").tag(sandbox.id)
            }
        }
    }

    private func methodPicker(connecting: Bool) -> some View {
        Picker("Method", selection: $selection.method) {
            Text("Choose a method").tag("")
            ForEach(offer?.methods ?? []) { method in
                Text(method.label + (!method.offerable ? " — unavailable" : ""))
                    .tag(method.name).disabled(!method.offerable || (connecting && method.auth_label == nil))
            }
        }
    }

    private var destructive: Bool {
        [.remove, .uninstall, .disconnect, .forget].contains(sheet.kind)
    }

    private var command: ManagementCommand? {
        switch sheet.kind {
        case .install:
            let trimmed = source.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : .install(trimmed)
        case .connect:
            let trimmed = label.trimmingCharacters(in: .whitespacesAndNewlines)
            guard let offer, let method, method.offerable, method.auth_label != nil,
                  !trimmed.isEmpty, !offer.connections.contains(where: { $0.label == trimmed }),
                  method.asks.allSatisfy({ !(values[$0] ?? "").isEmpty }) else { return nil }
            return .connect(offer.name, method: method.name, label: trimmed, values: values)
        case .grant:
            guard let offer else { return nil }
            return selection.command(offer: offer, sandboxes: model.currentSandboxes)
        case .forget:
            guard let offer, model.currentSandboxes.contains(where: { $0.id == selection.run }) else { return nil }
            return .forget(offer.name, run: selection.run)
        case .disconnect:
            guard let offer else { return nil }
            return .disconnect(offer.name, connection: sheet.connection)
        case .uninstall:
            guard let offer else { return nil }
            return .uninstall(offer.name)
        case .remove:
            guard let sandbox = sheet.sandbox,
                  model.currentSandboxes.contains(where: { $0.id == sandbox.id && $0.status == "exited" }) else { return nil }
            return .remove(sandbox.id)
        }
    }

    private func submit() {
        guard let command else { return }
        values = [:]
        Task {
            guard await model.manage(command, reviewing: offer) else { return }
            guard model.managementSheet?.id == sheet.id else { return }
            if sheet.returnToGrant, let current = model.management.connectors.first(where: { $0.name == offer?.name }) {
                model.managementSheet = ManagementSheet(kind: .grant, offer: current, run: sheet.run)
            } else { model.managementSheet = nil }
        }
    }

    private func chooseFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url { source = url.path }
    }
}

private struct DisclosureLine: View {
    let title: String
    let entries: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(LNSTheme.muted)
            Text(entries.isEmpty ? "None" : entries.joined(separator: "\n")).font(.callout).textSelection(.enabled)
        }
    }
}
