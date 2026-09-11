import SwiftUI
import LNSClient

@MainActor
struct SandboxList: View {
    @ObservedObject var model: DashboardModel
    @State private var search = ""
    @State private var status = ""

    private var sandboxes: [DashboardSandbox] {
        model.currentSandboxes.filter {
            (status.isEmpty || $0.status == status)
                && (search.isEmpty || "\($0.name) \($0.image) \($0.id)".localizedCaseInsensitiveContains(search))
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 16) {
                TextField("Find a sandbox", text: $search).textFieldStyle(.roundedBorder)
                Picker("Status", selection: $status) {
                    Text("All").tag("")
                    Text("Running").tag("running")
                    Text("Stopped").tag("exited")
                }.pickerStyle(.segmented).frame(maxWidth: 260)
            }.padding(16)
            Divider()
            List(sandboxes) { sandbox in
                HStack(spacing: 16) {
                    Image(systemName: "shippingbox")
                        .font(.title2).foregroundStyle(.secondary)
                        .frame(width: 36)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(sandbox.name).font(.headline).textSelection(.enabled)
                        Text(sandbox.image).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            .help(sandbox.image)
                        Text(sandbox.id).font(.caption2.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Spacer(minLength: 16)
                    Label(sandbox.statusLabel, systemImage: sandbox.status == "running" ? "circle.fill" : "stop.circle")
                        .font(.caption).foregroundStyle(sandbox.status == "running" ? Color.green : Color.secondary)
                        .frame(width: 85, alignment: .leading)
                    if sandbox.status == "running" {
                        Button("Stop") { perform(.stop(sandbox.id)) }
                            .help("Stop the workload and keep the sandbox for a later start")
                    } else {
                        Button("Start") { perform(.start(sandbox.id)) }
                    }
                    Menu {
                        Button("View Activity") { show(.audit, sandbox: sandbox) }
                        Button("View Approvals") { show(.approvals, sandbox: sandbox) }
                        Button("Grant Connector Access…") {
                            model.grantRun = sandbox.id
                            model.page = .connectors
                        }
                        Divider()
                        Button("Remove Sandbox…", role: .destructive) {
                            model.managementSheet = ManagementSheet(kind: .remove, sandbox: sandbox)
                        }.disabled(sandbox.status == "running")
                    } label: { Image(systemName: "ellipsis.circle").accessibilityLabel("Actions for \(sandbox.name)") }
                    .menuStyle(.borderlessButton).frame(width: 26)
                }
                .padding(.vertical, 10)
                .disabled(!model.connected || !model.management.connected || model.management.busy)
            }
            .listStyle(.inset)
            .overlay {
                if sandboxes.isEmpty {
                    VStack(spacing: 10) {
                        Image(systemName: "shippingbox").font(.largeTitle)
                        Text(model.connected ? "No sandboxes to show" : "Waiting for the service…").font(.headline)
                        Text(model.currentSandboxes.isEmpty ? "Sandboxes you run appear here. Stopped sandboxes stay until you remove them." : "Try another name or status.")
                            .multilineTextAlignment(.center).frame(maxWidth: 340)
                    }.foregroundStyle(.secondary).allowsHitTesting(false)
                }
            }
            if model.management.busy {
                HStack { ProgressView().controlSize(.small); Text("Updating sandbox or connector…").font(.caption) }.padding(10)
            }
        }
    }

    private func perform(_ command: ManagementCommand) {
        Task { _ = await model.manage(command) }
    }

    private func show(_ page: DashboardPage, sandbox: DashboardSandbox) {
        model.selectSandbox(sandbox.id)
        model.page = page
    }
}
