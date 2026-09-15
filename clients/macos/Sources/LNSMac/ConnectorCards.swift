import SwiftUI
import LNSClient

@MainActor
struct ConnectorCards: View {
    @ObservedObject var model: DashboardModel
    @State private var search = ""

    private var connectors: [ConnectorOffer] {
        model.management.connectors.filter {
            search.isEmpty || ([$0.name, $0.description ?? ""] + $0.serves).joined(separator: " ").localizedCaseInsensitiveContains(search)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            LNSPageHeader(title: "Connectors", subtitle: "Connect accounts and grant sandbox access.") {
                Button { model.managementSheet = ManagementSheet(kind: .install) } label: {
                    Label("Install Connector…", systemImage: "plus")
                }.buttonStyle(.borderedProminent).disabled(!model.management.connected || model.management.busy)
            }
            HStack {
                LNSSearchField(prompt: "Find a connector", text: $search).frame(maxWidth: 360)
                Spacer()
                Text("\(model.management.connectors.count) installed").font(.caption).foregroundStyle(LNSTheme.muted)
            }.padding(.horizontal, 24).padding(.bottom, 16)
            if let sandbox = model.currentSandboxes.first(where: { $0.id == model.grantRun }) {
                HStack {
                    Label("Choose a connector to grant to \(sandbox.name)", systemImage: "shippingbox")
                    Spacer()
                    Button("Clear") { model.grantRun = "" }.buttonStyle(.borderless)
                }.font(.callout).padding(12)
                    .background(LNSTheme.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                    .padding(.horizontal, 24).padding(.bottom, 16)
            }
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 320), spacing: 16, alignment: .top)], alignment: .leading, spacing: 16) {
                    ForEach(connectors, id: \.name) { connector in
                        ConnectorCard(model: model, connector: connector)
                    }
                }.padding(.horizontal, 24).padding(.bottom, 24)
            }
            .overlay {
                if connectors.isEmpty {
                    LNSEmptyState(
                        symbol: "link",
                        title: model.management.loading ? "Loading connectors…" : "No connectors to show",
                        message: search.isEmpty
                            ? "Install a connector to make it available to your sandboxes."
                            : "Try another name or destination."
                    )
                    .allowsHitTesting(false)
                }
            }
        }
        .background(LNSTheme.canvas)
        .task { if model.connected { await model.management.refresh() } }
    }
}

@MainActor
private struct ConnectorCard: View {
    @ObservedObject var model: DashboardModel
    let connector: ConnectorOffer

    private var canConnect: Bool { connector.methods.contains { $0.offerable && $0.auth_label != nil } }
    private var canGrant: Bool { !connector.grantOptions.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "link").font(.system(size: 18)).foregroundStyle(LNSTheme.accent)
                    .frame(width: 36, height: 36).background(LNSTheme.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                VStack(alignment: .leading, spacing: 5) {
                    Text(connector.name).font(.system(size: 14, weight: .semibold)).foregroundStyle(LNSTheme.heading)
                        .lineLimit(2).help(connector.name).textSelection(.enabled)
                    Text(connector.connections.isEmpty ? "No saved connections" : "\(connector.connections.count) saved \(connector.connections.count == 1 ? "connection" : "connections")")
                        .font(.caption).foregroundStyle(LNSTheme.muted)
                }
                Spacer()
                Menu {
                    Button("Forget Sandbox Decision…") { open(.forget) }.disabled(model.currentSandboxes.isEmpty)
                    Divider()
                    Button("Uninstall…", role: .destructive) { open(.uninstall) }
                } label: { Image(systemName: "ellipsis").accessibilityLabel("Actions for \(connector.name)") }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).frame(width: 24)
            }
            if let description = connector.description?.trimmingCharacters(in: .whitespacesAndNewlines), !description.isEmpty {
                Text(description)
                    .font(.callout).foregroundStyle(LNSTheme.muted)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !canGrant {
                Text(canConnect ? "Connect an account to grant sandbox access." : "This connector is unavailable in this version.")
                    .font(.callout).foregroundStyle(LNSTheme.muted)
            }
            if !connector.connections.isEmpty {
                Divider()
                ForEach(connector.connections) { connection in
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 4) {
                            Label(connection.label, systemImage: "person.crop.circle").font(.callout)
                                .lineLimit(1).help(connection.label)
                            Text(connection.authority.isEmpty ? "No authority reported" : connection.authority.joined(separator: ", "))
                                .font(.caption).foregroundStyle(LNSTheme.muted).textSelection(.enabled)
                        }
                        Spacer()
                        Button {
                            model.managementSheet = ManagementSheet(kind: .disconnect, offer: connector, connection: connection.label)
                        } label: { Image(systemName: "minus.circle").accessibilityLabel("Disconnect \(connection.label)") }
                        .buttonStyle(.borderless).help("Disconnect \(connection.label)")
                    }
                }
            }
            Divider()
            HStack {
                if canGrant && canConnect {
                    Button("New connection…") { open(.connect) }.buttonStyle(.borderless)
                }
                Spacer()
                if canGrant {
                    Button("Grant Access…") { open(.grant) }
                        .buttonStyle(.borderedProminent)
                        .disabled(model.currentSandboxes.isEmpty)
                        .help(model.currentSandboxes.isEmpty ? "Start a sandbox to grant it access." : "Choose a sandbox to grant access.")
                } else {
                    Button("Connect…") { open(.connect) }
                        .buttonStyle(.borderedProminent).disabled(!canConnect)
                }
            }
            .controlSize(.large)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .lnsPanel()
        .disabled(!model.connected || !model.management.connected || model.management.busy)
    }

    private func open(_ kind: ManagementSheet.Kind) {
        model.managementSheet = ManagementSheet(kind: kind, offer: connector, run: model.grantRun)
    }
}
