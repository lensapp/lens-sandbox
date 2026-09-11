import AppKit
import SwiftUI
import LNSClient

extension DashboardModel {
    func saveDefinition(_ sandbox: DashboardSandbox) {
        guard connected, !saving.busy else { return }
        let panel = NSSavePanel()
        panel.title = "Save Sandbox Definition"
        panel.prompt = "Save Definition"
        let cleaned = sandbox.name.lowercased().replacingOccurrences(of: "[^a-z0-9-]", with: "-", options: .regularExpression)
        let stem = String(cleaned.prefix(63)).trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        panel.nameFieldStringValue = "\(stem.isEmpty ? "sandbox" : stem).yaml"
        panel.message = "Save the resolved definition and persistent decisions. Mixins are folded in; connector grants stay with this sandbox. Choose a new file."
        guard panel.runModal() == .OK, let file = panel.url else { return }
        Task {
            if await saving.save(run: sandbox.id, to: file) {
                updateRecents { $0.saved(file.path) }
                notice = "Saved definition to \(file.path)"
            } else { notice = saving.error }
        }
    }
}

@MainActor
struct SandboxConfigurationView: View {
    @ObservedObject var model: DashboardModel
    let sandbox: DashboardSandbox
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                LNSPageHeading(title: sandbox.name, subtitle: "Configuration and decisions for this sandbox.")
                Spacer()
                Button("Save Definition…") { model.saveDefinition(sandbox) }.disabled(!model.connected || model.saving.busy)
                Button("Refresh") { Task { await model.refreshConfiguration() } }.disabled(!model.connected || model.configuration.loading)
            }
            if let value = model.configuration.configuration {
                ScrollView { ConfigurationContents(configuration: value, live: true, events: model.data.events, run: sandbox.id).frame(maxWidth: .infinity, alignment: .leading) }
            } else if model.configuration.loading {
                ProgressView("Reading configuration…").frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                Text(model.configuration.error ?? "Waiting for the service…").foregroundStyle(LNSTheme.warning)
                Spacer()
            }
            Divider()
            HStack {
                Text("Persistent decisions update as you answer requests.").font(.caption).foregroundStyle(LNSTheme.muted)
                Spacer()
                Button("Done") { model.inspectingSandbox = nil }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(24).frame(minWidth: 720, idealWidth: 850, minHeight: 620, idealHeight: 740)
        .lnsAppearance().task { await model.refreshConfiguration() }
    }
}

struct ConfigurationContents: View {
    let configuration: SandboxConfiguration
    var live = false
    var events: [DashboardEvent] = []
    var run = ""
    @State private var bySource = false

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            if live { decisions }
            if let sources = configuration.sources {
                sourceList(sources)
            } else {
                Text("This run did not retain source attribution. Its recorded configuration and current decisions are shown below.")
                    .font(.callout).foregroundStyle(LNSTheme.muted)
            }
            if !configuration.grants.isEmpty { grants }
            Picker("Configuration view", selection: $bySource) {
                Text("Effective Configuration").tag(false)
                Text("By Source").tag(true)
            }.pickerStyle(.segmented)
            if bySource, let sources = configuration.sources { contributions(sources) }
            else { effective }
        }.textSelection(.enabled)
    }

    private var decisions: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Your Decisions").font(.headline)
            if configuration.userDecisions.isEmpty {
                Text("No persistent network decisions. One-time answers appear in Approvals history.").font(.callout).foregroundStyle(LNSTheme.muted)
            }
            ruleRows(configuration.userDecisions)
            let extra = configuration.decisionSpec.filter { $0.key != "egress" }
            if !extra.isEmpty {
                DisclosureGroup("Other declarations in the decisions file") {
                    Text(SandboxConfiguration.display(extra)).font(.caption.monospaced())
                }
                Text("Tools, files, and setup are applied at startup; network decisions apply while running.").font(.caption).foregroundStyle(LNSTheme.muted)
            }
        }.padding(16).lnsPanel()
    }

    private func sourceList(_ sources: ConfigurationSources) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Definition").font(.headline)
            Text(sources.definition).font(.callout.monospaced()).fixedSize(horizontal: false, vertical: true)
            if !sources.mixins.isEmpty {
                Text("Mixins").font(.headline).padding(.top, 4)
                ForEach(Array(sources.mixins.enumerated()), id: \.offset) { _, source in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(source).font(.callout.monospaced()).fixedSize(horizontal: false, vertical: true)
                        Text(sources.added_mixins.contains(source) ? "Added by you" : "Included by definition or another mixin")
                            .font(.caption).foregroundStyle(LNSTheme.muted)
                    }
                }
            }
        }
    }

    private var grants: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Connector Grants").font(.headline)
            ForEach(configuration.grants, id: \.name) { grant in
                DisclosureGroup(grant.name) {
                    VStack(alignment: .leading, spacing: 6) {
                        ruleRows(configuration.rules.filter { $0.source == "Connector: \(grant.name)" })
                        ForEach(grant.variables, id: \.self) { Text("Variable: \($0)") }
                        ForEach(grant.files, id: \.self) { Text("File: \($0)") }
                    }.font(.callout)
                }
            }
        }
    }

    private var effective: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach([("image", "Image"), ("command", "Command"), ("resources", "Resources"), ("tools", "Tools"), ("env", "Environment"), ("workdir", "Working Directory"), ("user", "Workload User"), ("volumes", "Mounts"), ("filesets", "Files"), ("ports", "Published Ports"), ("credentials", "Credential Declarations"), ("scripts", "Setup Scripts")], id: \.0) { key, title in
                if let value = configuration.spec[key], populated(value) {
                    DisclosureGroup(title) {
                        Text(SandboxConfiguration.display(value)).font(.callout.monospaced())
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 6)
                    }
                }
            }
            Text("Network Rules").font(.headline)
            Text("Rules are listed in precedence order within each table. The first matching rule decides; destinations with no matching rule ask for approval.")
                .font(.caption).foregroundStyle(LNSTheme.muted)
                .fixedSize(horizontal: false, vertical: true)
            ruleRows(configuration.rules)
            ForEach(Array(configuration.rules.enumerated()), id: \.offset) { index, rule in
                if let source = configuration.overridingSource(for: index) {
                    Text("\(rule.destination): \(source) takes precedence over \(rule.source) for this rule’s scope.")
                        .font(.caption).foregroundStyle(LNSTheme.warning)
                }
            }
        }
    }

    private func contributions(_ sources: ConfigurationSources) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            ForEach(["the sandbox"] + sources.mixins, id: \.self) { source in
                let entries = sources.contributions.filter { $0.source == source }
                VStack(alignment: .leading, spacing: 8) {
                    Text(source == "the sandbox" ? "Definition" : source).font(.headline)
                    if entries.isEmpty { Text("No attributed contributions recorded.").font(.caption).foregroundStyle(LNSTheme.muted) }
                    ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                        VStack(alignment: .leading, spacing: 3) {
                            Text("\(entry.block.capitalized): \(contributionValue(entry))").font(.callout)
                            if let note = entry.note { Text(note).font(.caption).foregroundStyle(LNSTheme.muted) }
                            ForEach(Array((entry.displaced ?? []).enumerated()), id: \.offset) { _, old in
                                Text("Replaces \(old.summary) from \(old.source)").font(.caption).foregroundStyle(LNSTheme.warning)
                            }
                        }
                    }
                }
            }
        }
    }

    private func populated(_ value: Any) -> Bool {
        if value is NSNull { return false }
        if let items = value as? [Any] { return !items.isEmpty }
        if let fields = value as? [String: Any] { return !fields.isEmpty }
        return true
    }

    private func contributionValue(_ entry: ConfigurationContribution) -> String {
        guard entry.block == "tool", let tools = configuration.spec["tools"] as? [String] else { return entry.key }
        return tools.first { $0 == entry.key || $0.hasPrefix("\(entry.key)@") } ?? entry.key
    }

    private func ruleRows(_ rules: [ConfigurationRule]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(rules.enumerated()), id: \.offset) { _, rule in
                DisclosureGroup {
                    Text(SandboxConfiguration.display(rule.fields)).font(.caption.monospaced())
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 4)
                } label: {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("\(rule.verdict.capitalized) · \(rule.destination)")
                            .foregroundStyle(rule.verdict == "deny" ? LNSTheme.warning : LNSTheme.heading)
                        Text("\(rule.table.uppercased()) · \(rule.source)").font(.caption).foregroundStyle(LNSTheme.muted)
                        if let when = rule.latestApproval(in: events, run: run) {
                            Text("Latest matching approval: \(when)").font(.caption).foregroundStyle(LNSTheme.muted)
                        }
                    }
                }
            }
        }
    }
}
