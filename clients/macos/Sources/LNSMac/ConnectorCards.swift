import SwiftUI
import LNSClient

@MainActor
struct ConnectorCards: View {
    @ObservedObject var model: DashboardModel
    @State private var search = ""

    private var connectors: [ConnectorOffer] {
        model.management.connectors.filter {
            search.isEmpty || ([$0.name] + $0.serves).joined(separator: " ").localizedCaseInsensitiveContains(search)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 24) {
                LNSPageHeading(title: "Connectors", subtitle: "Connect accounts and grant access to your sandboxes.")
                Spacer()
                Button { model.managementSheet = ManagementSheet(kind: .install) } label: {
                    Label("Install Connector…", systemImage: "plus")
                }.buttonStyle(.borderedProminent).disabled(!model.management.connected || model.management.busy)
            }.padding(24)
            HStack {
                TextField("Find a connector", text: $search).textFieldStyle(.roundedBorder).frame(maxWidth: 360)
                Spacer()
                Text("\(connectors.count) installed").font(.caption).foregroundStyle(LNSTheme.muted)
            }.padding(.horizontal, 24).padding(.bottom, 8)
            if let sandbox = model.currentSandboxes.first(where: { $0.id == model.grantRun }) {
                HStack {
                    Label("Choose a connector to grant to \(sandbox.name)", systemImage: "shippingbox")
                    Spacer()
                    Button("Clear") { model.grantRun = "" }.buttonStyle(.borderless)
                }.font(.callout).padding(12)
                    .background(LNSTheme.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                    .padding(.horizontal, 24).padding(.top, 16)
            }
            ScrollView {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 320), spacing: 16)], alignment: .leading, spacing: 16) {
                    ForEach(connectors, id: \.name) { connector in
                        ConnectorCard(model: model, connector: connector)
                    }
                }.padding(24)
            }
            .overlay {
                if connectors.isEmpty {
                    VStack(spacing: 10) {
                        Image(systemName: "link").font(.system(size: 32, weight: .light)).foregroundStyle(LNSTheme.accent)
                        Text(model.management.loading ? "Loading connectors…" : "No connectors to show").font(.headline)
                        Text(search.isEmpty ? "Install a connector, connect an account if needed, then grant access to a sandbox." : "Try another name or destination.")
                            .multilineTextAlignment(.center).frame(maxWidth: 350)
                    }.foregroundStyle(LNSTheme.muted).allowsHitTesting(false)
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

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "link").font(.system(size: 18)).foregroundStyle(LNSTheme.accent)
                    .frame(width: 36, height: 36).background(LNSTheme.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 4))
                VStack(alignment: .leading, spacing: 5) {
                    Text(connector.name).font(.system(size: 15, weight: .semibold)).foregroundStyle(LNSTheme.heading).textSelection(.enabled)
                    Text(connector.connections.isEmpty ? "No saved connections" : "\(connector.connections.count) saved connection(s)")
                        .font(.caption).foregroundStyle(LNSTheme.muted)
                }
                Spacer()
                Menu {
                    Button("Forget Sandbox Decision…") { open(.forget) }.disabled(model.currentSandboxes.isEmpty)
                    Divider()
                    Button("Uninstall…", role: .destructive) { open(.uninstall) }
                } label: { Image(systemName: "ellipsis").accessibilityLabel("Actions for \(connector.name)") }
                .menuStyle(.borderlessButton).frame(width: 24)
            }
            Text(connector.serves.joined(separator: ", "))
                .font(.system(size: 11, design: .monospaced)).foregroundStyle(LNSTheme.muted).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10).background(LNSTheme.canvas, in: RoundedRectangle(cornerRadius: 4))
            VStack(alignment: .leading, spacing: 8) {
                ForEach(connector.methods) { method in
                    HStack(alignment: .top) {
                        Text(method.label).font(.callout)
                        Spacer()
                        LNSStatus(title: readiness(method), color: readinessColor(method))
                    }
                }
            }
            if !connector.connections.isEmpty {
                Divider()
                ForEach(connector.connections) { connection in
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 4) {
                            Label(connection.label, systemImage: "person.crop.circle").font(.callout)
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
            Spacer(minLength: 0)
            Divider()
            HStack {
                if connector.methods.contains(where: { $0.offerable && $0.auth_label != nil }) {
                    Button("Connect…") { open(.connect) }
                }
                Spacer()
                Button("Grant Access…") { open(.grant) }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.currentSandboxes.isEmpty || !connector.methods.contains(where: \.offerable))
            }
            if model.currentSandboxes.isEmpty {
                Text("Run a sandbox to grant it access.").font(.caption).foregroundStyle(LNSTheme.muted)
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, minHeight: 245, alignment: .topLeading)
        .lnsPanel()
        .disabled(!model.connected || !model.management.connected || model.management.busy)
    }

    private func readiness(_ method: ConnectorMethod) -> String {
        if !method.offerable { return "Unavailable in this version" }
        if method.auth_label == nil || connector.connections.contains(where: { $0.method == method.name }) { return "Ready to grant" }
        return "Connect first"
    }

    private func readinessColor(_ method: ConnectorMethod) -> Color {
        if !method.offerable { return LNSTheme.muted }
        return method.auth_label == nil || connector.connections.contains(where: { $0.method == method.name })
            ? LNSTheme.success : LNSTheme.warning
    }

    private func open(_ kind: ManagementSheet.Kind) {
        model.managementSheet = ManagementSheet(kind: kind, offer: connector, run: model.grantRun)
    }
}
