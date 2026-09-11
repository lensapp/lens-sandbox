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
            HStack(alignment: .top, spacing: 24) {
                LNSPageHeading(title: "Sandboxes", subtitle: "Run workloads locally. Control what they can access.")
                Spacer()
                Button { model.creatingSandbox = true } label: { Label("New Sandbox…", systemImage: "plus") }
                    .buttonStyle(.borderedProminent).disabled(!model.canCreateSandbox)
                    .help(model.creation == nil ? "This build includes only the interface. For a full demo, run make -C clients/macos package, then reopen dist/LNS.app." : "Run a local sandbox definition or a published reference")
            }.padding(24)
            HStack(spacing: 12) {
                Text("\(model.currentSandboxes.count) sandboxes").font(.system(size: 12, weight: .medium))
                LNSStatus(title: "\(model.currentSandboxes.filter { $0.status == "running" }.count) running", color: LNSTheme.success)
                LNSStatus(title: "\(model.currentSandboxes.filter { $0.status == "exited" }.count) stopped")
                Spacer()
            }.padding(.horizontal, 24)
            HStack(spacing: 16) {
                TextField("Find a sandbox", text: $search).textFieldStyle(.roundedBorder)
                Picker("Status", selection: $status) {
                    Text("All").tag("")
                    Text("Running").tag("running")
                    Text("Stopped").tag("exited")
                }.pickerStyle(.segmented).frame(maxWidth: 260)
            }.padding(.horizontal, 24).padding(.vertical, 16)
            HStack {
                Text("SANDBOX / IMAGE")
                Spacer()
                Text("STATUS").frame(width: 100, alignment: .leading)
                Text("ACTIONS").frame(width: 96, alignment: .trailing)
            }
            .font(.system(size: 10, weight: .semibold)).tracking(0.8)
            .foregroundStyle(LNSTheme.muted).padding(.horizontal, 20).padding(.vertical, 12)
            .background(LNSTheme.raised)
            .padding(.horizontal, 24)
            List(sandboxes) { sandbox in
                sandboxRow(sandbox)
            }
            .listStyle(.inset)
            .scrollContentBackground(.hidden)
            .background(LNSTheme.surface)
            .overlay(Rectangle().strokeBorder(LNSTheme.border, lineWidth: 1).allowsHitTesting(false))
            .overlay {
                if sandboxes.isEmpty {
                    VStack(spacing: 10) {
                        Image(systemName: "shippingbox").font(.system(size: 32, weight: .light)).foregroundStyle(LNSTheme.accent)
                        Text(model.connected ? "No sandboxes to show" : "Waiting for the service…").font(.headline)
                        Text(model.currentSandboxes.isEmpty ? "Sandboxes you run appear here. Stopped sandboxes stay until you remove them." : "Try another name or status.")
                            .multilineTextAlignment(.center).frame(maxWidth: 340)
                        if model.currentSandboxes.isEmpty {
                            Button("New Sandbox…") { model.creatingSandbox = true }.disabled(!model.canCreateSandbox)
                        }
                    }.foregroundStyle(LNSTheme.muted)
                }
            }
            .padding(.horizontal, 24).padding(.bottom, 24)
            if model.management.busy {
                HStack { ProgressView().controlSize(.small); Text("Updating sandbox or connector…").font(.caption) }.padding(10)
            }
        }
    }

    private func sandboxRow(_ sandbox: DashboardSandbox) -> some View {
        HStack(spacing: 16) {
            Image(systemName: "shippingbox")
                .font(.system(size: 18)).foregroundStyle(LNSTheme.muted)
                .frame(width: 32, height: 36)
            VStack(alignment: .leading, spacing: 5) {
                Text(sandbox.name).lineLimit(1).help(sandbox.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(LNSTheme.heading).textSelection(.enabled)
                Text(sandbox.image).font(.caption.monospaced()).foregroundStyle(LNSTheme.muted).lineLimit(1)
                    .help(sandbox.image)
                Text(sandbox.id).lineLimit(1).help(sandbox.id).font(.caption2.monospaced()).foregroundStyle(LNSTheme.muted).textSelection(.enabled)
            }
            Spacer(minLength: 16)
            LNSStatus(title: sandbox.statusLabel, color: sandbox.status == "running" ? LNSTheme.success : LNSTheme.muted)
                .frame(width: 100, alignment: .leading)
            if sandbox.status == "running" {
                Button("Stop") { perform(.stop(sandbox.id)) }
                    .frame(width: 60)
                    .help("Stop the workload and keep the sandbox for a later start")
            } else {
                Button("Start") { perform(.start(sandbox.id)) }
                    .frame(width: 60)
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
        .listRowBackground(LNSTheme.surface)
        .disabled(!model.connected || !model.management.connected || model.management.busy)
    }

    private func perform(_ command: ManagementCommand) {
        Task { _ = await model.manage(command) }
    }

    private func show(_ page: DashboardPage, sandbox: DashboardSandbox) {
        model.selectSandbox(sandbox.id)
        model.page = page
    }
}
