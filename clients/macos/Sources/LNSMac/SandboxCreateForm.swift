import AppKit
import SwiftUI
import LNSClient

@MainActor
struct SandboxCreateForm: View {
    @ObservedObject var model: DashboardModel
    @State private var draft = SandboxDraft()
    @State private var attempted = false
    @State private var registryLogin = false
    @State private var addingMixin = false

    private var busy: Bool { model.creation?.busy == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            LNSPageHeading(title: "New Sandbox", subtitle: "Start from a definition. It keeps running when you close the window.")
            ScrollViewReader { scroll in
              ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    fields.disabled(busy)
                    if model.preview.loading { ProgressView("Resolving definition and mixins…") }
                    if let error = model.preview.error {
                        Text(error).foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    }
                    if let configuration = model.preview.configuration {
                        Divider()
                        Text("Review Configuration").font(.headline).id("configurationPreview")
                        ConfigurationContents(configuration: configuration)
                    }
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
              .onChange(of: model.preview.configuration) { value in
                  if value != nil { withAnimation { scroll.scrollTo("configurationPreview", anchor: .top) } }
              }
            }
            Divider()
            HStack {
                if busy { ProgressView().controlSize(.small); Text("Starting sandbox…").font(.caption) }
                Spacer()
                Button("Cancel") { model.creatingSandbox = false }
                    .keyboardShortcut(.cancelAction).disabled(busy)
                Button("Start Sandbox", action: start)
                    .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
                    .disabled(!model.canCreateSandbox || (try? draft.arguments()) == nil || model.preview.configuration == nil || model.preview.loading)
            }
        }
        .padding(24).frame(width: 720, height: 760)
        .controlSize(.large)
        .lnsAppearance()
        .interactiveDismissDisabled(busy)
        .sheet(isPresented: $registryLogin) { RegistryLoginForm(model: model) }
        .sheet(isPresented: $addingMixin) {
            MixinPicker(model: model, selected: draft.mixins) { source in
                draft.mixins.append(source); addingMixin = false
            }
        }
        .onAppear { model.preview.clear() }
        .onChange(of: [draft.source] + draft.mixins) { _ in
            model.preview.clear(); draft.allowSetup = false; attempted = false
        }
        .onDisappear { model.preview.clear() }
    }

    private var fields: some View {
        VStack(alignment: .leading, spacing: 16) {
            Picker("Definition", selection: Binding(get: { draft.kind }, set: { draft.kind = $0; draft.source = ""; draft.allowSetup = false })) {
                Text("Local File or Folder").tag(SandboxDraft.Source.local)
                Text("Published Reference").tag(SandboxDraft.Source.published)
            }.pickerStyle(.segmented)
            LNSFormField(title: draft.kind == .local ? "Definition path" : "Published reference") {
                HStack {
                    TextField(draft.kind == .local ? "/path/to/lns.yaml" : "ghcr.io/team/agent:latest", text: $draft.source)
                        .textFieldStyle(.roundedBorder)
                    if draft.kind == .local { Button("Choose…", action: chooseDefinition) }
                }
            }
            if !model.recents.definitions.isEmpty {
                Menu("Recently Used Definitions") {
                    ForEach(model.recents.definitions, id: \.self) { source in
                        Button(source) {
                            draft.kind = source.hasPrefix("/") ? .local : .published
                            draft.source = source
                        }
                    }
                    Divider()
                    Button("Clear Recent Definitions") { model.updateRecents { recent in
                        for source in recent.definitions { recent.removeDefinition(source) }
                    } }
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
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("Mixins").font(.headline)
                    Spacer()
                    Button("Add Mixin…") { addingMixin = true }
                }
                Text("Reusable additions: tools, network rules, files, and setup. Later additions take precedence.")
                    .font(.caption).foregroundStyle(LNSTheme.muted)
                ForEach(Array(draft.mixins.enumerated()), id: \.offset) { index, source in
                    HStack {
                        Text(source).font(.callout.monospaced()).lineLimit(2).help(source)
                        Spacer()
                        Button { draft.mixins.swapAt(index, index - 1) } label: { Image(systemName: "arrow.up") }
                            .disabled(index == 0).accessibilityLabel("Move \(source) earlier")
                        Button { draft.mixins.swapAt(index, index + 1) } label: { Image(systemName: "arrow.down") }
                            .disabled(index == draft.mixins.count - 1).accessibilityLabel("Move \(source) later")
                        Button { draft.mixins.remove(at: index) } label: { Image(systemName: "minus.circle") }
                            .accessibilityLabel("Remove \(source)")
                    }
                }
            }
            Button("Review Configuration") {
                let selected = draft
                Task { await model.preview.read(.preview(selected)) }
            }.disabled((try? draft.arguments()) == nil || model.preview.loading || !model.connected)
            Button("Sign In to a Registry…") { registryLogin = true }
                .disabled(!model.registries.connected || model.registries.busy)
            Toggle("Allow this definition’s setup and declared host access", isOn: $draft.allowSetup)
                .disabled(model.preview.configuration == nil)
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
        guard let sources = model.preview.configuration?.sources else { return }
        var pinned = submitted
        if submitted.kind == .published { pinned.source = sources.definition }
        pinned.mixins = sources.added_mixins
        Task {
            let started = await creation.start(pinned)
            if started { model.updateRecents { $0.record(submitted) } }
            await model.refresh()
            if started { model.page = .sandboxes; model.creatingSandbox = false }
        }
    }
}

@MainActor
struct MixinPicker: View {
    @ObservedObject var model: DashboardModel
    let selected: [String]
    let add: (String) -> Void
    @State private var source = ""
    @Environment(\.dismiss) private var dismiss
    private var candidate: String { source.trimmingCharacters(in: .whitespacesAndNewlines) }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            LNSPageHeading(title: "Add Mixin", subtitle: "Choose a reusable addition to this sandbox.")
            LNSFormField(title: "Local path or published reference") {
                TextField("ghcr.io/team/tools:latest", text: $source).textFieldStyle(.roundedBorder)
            }
            Button("Choose Local File or Folder…") {
                let panel = NSOpenPanel()
                panel.canChooseFiles = true; panel.canChooseDirectories = true; panel.allowsMultipleSelection = false
                panel.prompt = "Choose Mixin"
                if panel.runModal() == .OK, let url = panel.url { source = url.path }
            }
            Text("Recently Used").font(.headline)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if model.recents.mixins.isEmpty {
                        Text("Mixins appear here after a successful launch.").foregroundStyle(LNSTheme.muted)
                    }
                    ForEach(model.recents.mixins, id: \.self) { recent in
                        HStack {
                            Button(recent) { source = recent }.buttonStyle(.plain).multilineTextAlignment(.leading)
                            Spacer()
                            Button { model.updateRecents { $0.removeMixin(recent) } } label: { Image(systemName: "xmark") }
                                .buttonStyle(.plain).accessibilityLabel("Remove \(recent) from recents")
                        }
                    }
                }
            }
            Divider()
            HStack {
                if selected.contains(candidate) { Text("Already added").font(.caption) }
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Add Mixin") { add(candidate) }.buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled((try? SandboxDraft.validateMixin(candidate)) == nil || selected.contains(candidate))
            }
        }.padding(24).frame(width: 560, height: 450).lnsAppearance()
    }
}
