import AppKit
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
            LNSPageHeader(title: "Sandboxes", subtitle: "Manage running and stopped workloads.") {
                Button { model.creatingSandbox = true } label: { Label("New Sandbox…", systemImage: "plus") }
                    .buttonStyle(.borderedProminent).disabled(!model.canCreateSandbox)
                    .help(model.creation == nil ? "This build includes only the interface. For a full demo, run make -C clients/macos package, then reopen dist/LNS.app." : "Run a local sandbox definition or a published reference")
            }
            HStack(spacing: 12) {
                LNSSearchField(prompt: "Find a sandbox", text: $search).frame(maxWidth: 360)
                Spacer(minLength: 0)
                Picker("Status", selection: $status) {
                    Text("All").tag("")
                    Text("Running").tag("running")
                    Text("Stopped").tag("exited")
                }.pickerStyle(.segmented).fixedSize()
            }.padding(.horizontal, 24).padding(.bottom, 16)
            VStack(spacing: 0) {
                HStack(spacing: 16) {
                    Text("Sandbox").frame(maxWidth: .infinity, alignment: .leading)
                    Text("Status").frame(width: 100, alignment: .leading)
                    Text("Actions").frame(width: 102, alignment: .trailing)
                }
                .font(.system(size: 11, weight: .medium)).foregroundStyle(LNSTheme.muted)
                .padding(.horizontal, 16).padding(.vertical, 10)
                .background(LNSTheme.raised)
                Divider()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(sandboxes) { sandbox in
                            sandboxRow(sandbox)
                            Divider().padding(.horizontal, 16)
                        }
                    }
                }
                .accessibilityLabel("Sandboxes")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .overlay {
                    if sandboxes.isEmpty {
                        LNSEmptyState(
                            symbol: "shippingbox",
                            title: model.connected ? "No sandboxes to show" : "Waiting for the service…",
                            message: model.currentSandboxes.isEmpty
                                ? "Start with New Sandbox. Stopped sandboxes stay here until you remove them."
                                : "Try another name or status."
                        )
                    }
                }
                Divider()
                HStack(spacing: 12) {
                    Text("\(sandboxes.count) of \(model.currentSandboxes.count) sandboxes")
                    Spacer()
                    Text("\(model.currentSandboxes.filter { $0.status == "running" }.count) running")
                }
                .font(.system(size: 11)).monospacedDigit().foregroundStyle(LNSTheme.muted)
                .padding(.horizontal, 16).padding(.vertical, 10)
            }
            .clipShape(RoundedRectangle(cornerRadius: 4)).lnsPanel()
            .padding(.horizontal, 24).padding(.bottom, 24)
            if model.management.busy {
                HStack { ProgressView().controlSize(.small); Text("Updating sandbox or connector…").font(.caption) }.padding(10)
            }
        }
    }

    private func sandboxRow(_ sandbox: DashboardSandbox) -> some View {
        HStack(spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Button(sandbox.name) { model.inspect(sandbox) }
                    .buttonStyle(.plain).lineLimit(1).help("Inspect \(sandbox.name)").font(.system(size: 13, weight: .semibold)).foregroundStyle(LNSTheme.heading)
                Text(sandbox.image).font(.caption.monospaced()).foregroundStyle(LNSTheme.muted).lineLimit(1)
                    .help(sandbox.image)
                Text(sandbox.id).lineLimit(1).help(sandbox.id).font(.caption2.monospaced()).foregroundStyle(LNSTheme.muted).textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            LNSStatus(title: sandbox.statusLabel, color: sandbox.status == "running" ? LNSTheme.success : LNSTheme.muted)
                .frame(width: 100, alignment: .leading)
            HStack(spacing: 8) {
                if sandbox.status == "running" {
                    Button("Stop") { perform(.stop(sandbox.id)) }
                        .frame(width: 60)
                        .help("Stop the workload and keep the sandbox for a later start")
                } else {
                    Button("Start") { perform(.start(sandbox.id)) }
                        .frame(width: 60)
                }
                Menu {
                    Button("View Configuration…") { model.inspect(sandbox) }
                    Button("Save Definition…") { model.saveDefinition(sandbox) }.disabled(model.saving.busy)
                    Divider()
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
                .menuStyle(.borderlessButton).menuIndicator(.hidden).frame(width: 26)
            }
            .frame(width: 102, alignment: .trailing)
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(sandbox.name)
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
