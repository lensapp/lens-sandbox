import SwiftUI
import LNSClient

struct ConnectorGrant: View {
    let offer: ConnectorOffer
    var showsDecline = true
    var showsDetails = true
    var details: (() -> Void)?
    let respond: (ApprovalAction) -> Void
    @State private var draft = LiveConnectorDraft()

    private var newMethods: [ConnectorMethod] { offer.methods.filter { $0.offerable && $0.auth_label != nil } }
    private var option: ConnectorGrantOption? { offer.grantOptions.first { $0.id == draft.selection } }
    private var selected: ConnectorMethod? {
        draft.newMethod(in: offer) ?? offer.methods.first { $0.name == option?.method }
    }
    private var connection: ConnectorConnection? { offer.connections.first { $0.label == option?.connection } }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("Connection", selection: Binding(get: { draft.selection }, set: { draft.choose($0) })) {
                Text("Choose a connection").tag("")
                ForEach(offer.grantOptions) { option in Text(option.label).tag(option.id) }
                if !newMethods.isEmpty { Divider() }
                ForEach(newMethods) { method in
                    Text(newMethods.count == 1 ? "New connection…" : "New connection · \(method.label)…")
                        .tag("new:\(method.name)")
                }
            }
            if let method = draft.newMethod(in: offer) {
                LNSFormField(title: "Connection name") {
                    TextField("e.g. work", text: $draft.name)
                }
                ForEach(method.asks, id: \.self) { field in
                    LNSFormField(title: field) {
                        SecureField(field, text: Binding(get: { draft.values[field] ?? "" }, set: { draft.values[field] = $0 }))
                    }
                }
                if offer.connections.contains(where: { $0.label == draft.name.trimmingCharacters(in: .whitespacesAndNewlines) }) {
                    Text("That name is already saved. Choose another name.")
                        .font(.system(size: 11)).foregroundStyle(LNSTheme.warning)
                }
                Text("Credentials stay outside the sandbox.").font(.system(size: 11)).foregroundStyle(LNSTheme.muted)
            }
            if showsDetails {
                if let selected { disclosure(selected) }
                else {
                    ForEach(offer.methods.filter(\.offerable)) { method in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(method.label).font(.system(size: 12, weight: .medium))
                            disclosure(method)
                        }
                    }
                }
            }
            HStack {
                if let details {
                    Button("View details", action: details).buttonStyle(.borderless)
                }
                if showsDecline {
                    Button("Skip Connector") { respond(.decline) }.buttonStyle(LNSApprovalActionStyle())
                }
                Spacer()
                Button(draft.newMethod(in: offer) == nil ? "Grant Access" : "Connect & Grant") {
                    guard let action = draft.action(offer: offer) else { return }
                    draft.values = [:]
                    respond(action)
                }
                .buttonStyle(LNSApprovalActionStyle(prominent: true)).disabled(draft.action(offer: offer) == nil)
            }
        }
        .controlSize(.large).textFieldStyle(.roundedBorder)
        .onDisappear { draft.values = [:] }
    }

    private func disclosure(_ method: ConnectorMethod) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            if let connection { disclosureLine("Connection authority", connection.authority) }
            disclosureLine("Opens", method.opens)
            disclosureLine("Writes", method.writes)
            disclosureLine("Sets", method.env + method.credentials)
            if let overrides = method.overrides {
                disclosureLine("Overrides deny rules", overrides)
            } else {
                Text("Deny-rule overrides could not be checked.").foregroundStyle(LNSTheme.warning)
            }
            if let help = method.help { Text(help) }
        }
        .font(.system(size: 12)).textSelection(.enabled)
        .padding(12).frame(maxWidth: .infinity, alignment: .leading)
        .background(LNSTheme.canvas, in: RoundedRectangle(cornerRadius: 4))
    }

    private func disclosureLine(_ title: String, _ entries: [String]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 11)).foregroundStyle(LNSTheme.muted)
            Text(entries.isEmpty ? "None" : entries.joined(separator: "\n"))
        }
    }
}
