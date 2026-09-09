import SwiftUI
import LNSClient

struct ApprovalList: View {
    @ObservedObject var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let error = model.connectionNotice {
                    Label(error, systemImage: "wifi.exclamationmark")
                        .foregroundStyle(.secondary)
                    Text("Start the service with lns service start.")
                        .font(.callout)
                }
                if let notice = model.notice {
                    Label(notice, systemImage: "exclamationmark.triangle")
                        .textSelection(.enabled)
                }
                ForEach(Array(model.snapshot.notices.enumerated()), id: \.offset) { _, notice in
                    Label(notice, systemImage: "exclamationmark.triangle")
                        .textSelection(.enabled)
                }
                if model.connected && model.snapshot.approvals.isEmpty {
                    Label("No requests waiting for approval", systemImage: "checkmark.shield")
                        .foregroundStyle(.secondary)
                }
                ForEach(model.snapshot.approvals) { approval in
                    ApprovalCard(approval: approval) { action in model.respond(to: approval, with: action) }
                        .disabled(!model.connected || approval.submitting || model.busy.contains(approval.id))
                    Divider()
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct ApprovalCard: View {
    let approval: LiveApproval
    let respond: (ApprovalAction) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(approval.run ?? "Sandbox").font(.subheadline).foregroundStyle(.secondary)
            Text(approval.host).font(.title2).textSelection(.enabled)
            Text(approval.action).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
            if approval.raw {
                Label("LNS cannot inspect this traffic.", systemImage: "eye.slash")
                    .foregroundStyle(.orange)
            }
            if !approval.waiting {
                Text("The request stopped waiting. Connecting still applies to its next attempt.")
                    .foregroundStyle(.secondary)
            }
            if let offer = approval.offer {
                ConnectorGrant(offer: offer, respond: respond)
            } else {
                HStack {
                    Button("Allow Once") { respond(.allowOnce) }
                    Button("Always Allow") { respond(.allowAlways) }
                }
                HStack {
                    Button("Deny Once") { respond(.denyOnce) }
                    Button("Always Deny") { respond(.denyAlways) }
                }
            }
            Button("Dismiss Request") { respond(.dismiss) }
                .help("Fail this held request without recording a decision.")
        }
        .buttonStyle(.bordered)
        .accessibilityElement(children: .contain)
    }
}

struct ConnectorGrant: View {
    let offer: ConnectorOffer
    let respond: (ApprovalAction) -> Void
    @State private var methodName = ""
    @State private var account = ""
    @State private var label = ""
    @State private var values: [String: String] = [:]

    private var selected: ConnectorMethod? { offer.methods.first { $0.name == methodName } }
    private var held: [ConnectorConnection] { offer.connections.filter { $0.method == methodName } }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Connect this sandbox to \(offer.name)").font(.headline)
            Picker("Method", selection: $methodName) {
                Text("Choose a method").tag("")
                ForEach(offer.methods) { method in
                    Text(method.label).tag(method.name).disabled(!method.offerable)
                }
            }
            .onChange(of: methodName) { _ in account = ""; label = ""; values = [:] }
            if let method = selected, method.offerable {
                disclosure(method)
                if method.auth_label != nil {
                    connectionFields(method)
                }
                Button("Grant Access") { grant(method) }
                    .disabled(!canGrant(method))
            }
            Button("Decline Connector") { respond(.decline) }
        }
    }

    private func disclosure(_ method: ConnectorMethod) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            disclosureLine("Opens", method.opens)
            disclosureLine("Writes", method.writes)
            disclosureLine("Sets", method.env + method.credentials)
            if let overrides = method.overrides {
                disclosureLine("Overrides deny rules", overrides)
            } else {
                Text("Deny-rule overrides could not be checked.").foregroundStyle(.orange)
            }
            if let help = method.help { Text(help) }
        }
        .font(.callout)
        .textSelection(.enabled)
    }

    private func disclosureLine(_ title: String, _ entries: [String]) -> some View {
        Text("\(title): \(entries.isEmpty ? "None" : entries.joined(separator: ", "))")
    }

    private func connectionFields(_ method: ConnectorMethod) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Picker("Connection", selection: $account) {
                Text("New connection").tag("")
                ForEach(held) { connection in Text(connection.label).tag(connection.label) }
            }
            if let connection = held.first(where: { $0.label == account }) {
                disclosureLine("Authority", connection.authority).textSelection(.enabled)
            } else {
                TextField("Connection name", text: $label)
                ForEach(method.asks, id: \.self) { field in
                    SecureField(field, text: Binding(
                        get: { values[field] ?? "" },
                        set: { values[field] = $0 }
                    ))
                }
                Text("Credentials stay outside the workload.").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func canGrant(_ method: ConnectorMethod) -> Bool {
        if method.auth_label == nil { return true }
        if !account.isEmpty { return held.contains { $0.label == account } }
        return !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !offer.connections.contains { $0.label == label }
            && method.asks.allSatisfy { !(values[$0] ?? "").isEmpty }
    }

    private func grant(_ method: ConnectorMethod) {
        let connection: ConnectionChoice
        if method.auth_label == nil { connection = .none }
        else if !account.isEmpty { connection = .held(label: account) }
        else { connection = .new(label: label, values: values) }
        respond(.grant(method: method.name, connection: connection))
        values = [:]
    }
}
