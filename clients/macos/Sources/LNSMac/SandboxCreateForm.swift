import AppKit
import SwiftUI
import LNSClient

@MainActor
struct SandboxCreateForm: View {
    @ObservedObject var model: DashboardModel
    @State private var draft = SandboxDraft()
    @State private var attempted = false
    @State private var registryLogin = false

    private var busy: Bool { model.creation?.busy == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            LNSPageHeading(title: "New Sandbox", subtitle: "Start from a definition. It keeps running when you close the window.")
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    fields.disabled(busy)
                    if attempted, let error = model.creation?.error {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    }
                    if attempted, let output = model.creation?.output, !output.isEmpty {
                        Text("Launch details").font(.headline)
                        Text(output).font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12).lnsPanel()
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            HStack {
                if busy { ProgressView().controlSize(.small); Text("Starting sandbox…").font(.caption) }
                Spacer()
                Button("Cancel") { model.creatingSandbox = false }
                    .keyboardShortcut(.cancelAction).disabled(busy)
                Button("Start Sandbox", action: start)
                    .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
                    .disabled(!model.canCreateSandbox || (try? draft.arguments()) == nil)
            }
        }
        .padding(24).frame(width: 580, height: 620)
        .controlSize(.large)
        .lnsAppearance()
        .interactiveDismissDisabled(busy)
        .sheet(isPresented: $registryLogin) { RegistryLoginForm(model: model) }
    }

    private var fields: some View {
        VStack(alignment: .leading, spacing: 16) {
            Picker("Definition", selection: $draft.kind) {
                Text("Local File or Folder").tag(SandboxDraft.Source.local)
                Text("Published Reference").tag(SandboxDraft.Source.published)
            }.pickerStyle(.segmented)
                .onChange(of: draft.kind) { _ in draft.source = ""; draft.allowSetup = false }
            LNSFormField(title: draft.kind == .local ? "Definition path" : "Published reference") {
                HStack {
                    TextField(draft.kind == .local ? "/path/to/lns.yaml" : "ghcr.io/team/agent:latest", text: $draft.source)
                        .textFieldStyle(.roundedBorder)
                    if draft.kind == .local { Button("Choose…", action: chooseDefinition) }
                }
            }
            LNSFormField(title: "Sandbox name (optional)") {
                TextField("Optional name", text: $draft.name).textFieldStyle(.roundedBorder)
            }
            if let validationError {
                Text(validationError).font(.caption).foregroundStyle(LNSTheme.warning)
            }
            Text("The definition supplies the workload, resources, and network policy.")
                .font(.caption).foregroundStyle(LNSTheme.muted)
            Button("Sign In to a Registry…") { registryLogin = true }
                .disabled(!model.registries.connected || model.registries.busy)
            Toggle("Allow this definition’s setup and declared host access", isOn: $draft.allowSetup)
            Text("Accepts declared tool installers, setup scripts, file mounts, and host access. Leave this off to see any required consent in the launch details before trying again.")
                .font(.caption).foregroundStyle(LNSTheme.muted)
        }
    }

    private var validationError: String? {
        guard !draft.source.isEmpty else { return nil }
        do { _ = try draft.arguments(); return nil }
        catch { return error.localizedDescription }
    }

    private func chooseDefinition() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose Definition"
        if panel.runModal() == .OK, let url = panel.url { draft.source = url.path }
    }

    private func start() {
        guard model.canCreateSandbox, let creation = model.creation else { return }
        attempted = true
        let submitted = draft
        Task {
            let started = await creation.start(submitted)
            await model.refresh()
            if started { model.page = .sandboxes; model.creatingSandbox = false }
        }
    }
}
