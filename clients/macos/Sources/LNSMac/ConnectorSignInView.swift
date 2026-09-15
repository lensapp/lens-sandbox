import AppKit
import SwiftUI
import LNSClient

struct ConnectorDisclosure: View {
    let method: ConnectorMethod
    let digest: String
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let disclosure = method.codeDisclosure {
                Text(disclosure)
                Text("Installed digest: \(digest)").font(.caption.monospaced())
                Text(method.hosts.isEmpty ? "it may contact no hosts." : "May contact: \(method.hosts.joined(separator: ", "))")
            }
            if let oauth = method.oauth {
                Text("Authentication destinations: \(oauth.destinations.joined(separator: ", "))")
                ForEach(oauth.scope_options) { option in Text("\(option.label): \(option.permissions)") }
                if let callback = oauth.callback { Text("Callback: \(callback)") }
            }
        }.font(.callout).textSelection(.enabled)
    }
}

struct ConnectorRound: View {
    let connector: String
    let message: String
    let fields: [ConnectorField]
    let fromCode: Bool
    let progress: OAuthProgress?
    @Binding var answers: ConnectAnswers
    let submit: () -> Void
    let openBrowser: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let progress { oauth(progress) }
            if !message.isEmpty { Text(fromCode ? "\(connector) says: \(message)" : message).textSelection(.enabled) }
            if fromCode, !fields.isEmpty { Text("\(connector) asks, in its own words:") }
            ForEach(fields) { field in
                LNSFormField(title: field.label) {
                    let value = Binding(get: { answers.values[field.name] ?? "" }, set: { answers.values[field.name] = $0 })
                    if field.secret { SecureField(field.label, text: value) }
                    else { TextField(field.label, text: value) }
                }
            }
            if progress == nil || isSelecting {
                Button(isSelecting ? "Use Selected Permissions" : "Continue", action: submit)
                    .buttonStyle(.borderedProminent).disabled(!answers.ready(fields: fields, progress: progress))
            }
        }.textFieldStyle(.roundedBorder)
    }
    private var isSelecting: Bool {
        if case .selectingScopes = progress { return true }
        return false
    }
    @ViewBuilder private func oauth(_ progress: OAuthProgress) -> some View {
        switch progress {
        case let .selectingScopes(options):
            Text("Choose permissions").font(.headline)
            Picker("Permissions", selection: Binding(get: { answers.values["scopeOption"] ?? "" }, set: { answers.values["scopeOption"] = $0 })) {
                Text("Choose permissions…").tag("")
                ForEach(options) { option in Text("\(option.label): \(option.permissions)").tag(option.name) }
            }
        case let .starting(destinations, scopes):
            ProgressView("Preparing authorization…")
            Text("Authentication destinations: \(destinations.joined(separator: ", "))")
            Text("Requested permissions: \(scopes.isEmpty ? "Provider default permissions" : scopes.joined(separator: " "))")
        case let .deviceAuthorization(uri, code):
            Text("Sign in at \(uri)").textSelection(.enabled)
            Text("Code: \(code)").font(.title3.monospaced()).textSelection(.enabled)
            HStack {
                Button("Open Browser", action: openBrowser)
                Button("Copy Code") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(code, forType: .string) }
            }
            ProgressView("Waiting for authorization…")
        case let .waitingForBrowser(endpoint, redirect):
            Text("Sign in at \(endpoint)").textSelection(.enabled)
            Text("Waiting for callback at \(redirect)").font(.caption).textSelection(.enabled)
            Button("Open Browser", action: openBrowser)
            ProgressView("Waiting for authorization…")
        case .canceled: Text("Authorization canceled.")
        case .expired: Text("Authorization expired. Close this form and connect again.")
        }
    }
}

@MainActor
final class ConnectFormModel: ObservableObject {
    let session: ConnectSession
    init(session: ConnectSession) {
        self.session = session
        session.onChange = { [weak self] in self?.objectWillChange.send() }
    }
}

@MainActor
struct AccountConnectForm: View {
    @StateObject private var model: ConnectFormModel
    let offer: ConnectorOffer
    let connected: Bool
    let finish: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var method = ""
    @State private var label = ""
    @State private var answers = ConnectAnswers()
    private var session: ConnectSession { model.session }
    private var selected: ConnectorMethod? { offer.methods.first { $0.name == method } }

    init(session: ConnectSession, offer: ConnectorOffer, connected: Bool, method: String = "", label: String = "", finish: @escaping (String) -> Void) {
        _model = StateObject(wrappedValue: ConnectFormModel(session: session))
        self.offer = offer; self.connected = connected; self.finish = finish
        _method = State(initialValue: method); _label = State(initialValue: label)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Connect Account").font(.title2.weight(.semibold))
            Text(offer.name).font(.headline)
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if let selected { ConnectorDisclosure(method: selected, digest: offer.digest) }
                    if let ask = session.ask {
                        ConnectorRound(connector: offer.name, message: ask.message, fields: ask.fields, fromCode: ask.from_code,
                            progress: nil, answers: $answers, submit: submit, openBrowser: openBrowser)
                    } else if let progress = session.progress {
                        ConnectorRound(connector: offer.name, message: "", fields: [], fromCode: false,
                            progress: progress, answers: $answers, submit: submit, openBrowser: openBrowser)
                    } else if let completed = session.completed {
                        Text(completed)
                    } else if session.session == nil {
                        Picker("Method", selection: $method) {
                            Text("Choose a method").tag("")
                            ForEach(offer.methods.filter { $0.offerable && $0.auth_label != nil }) { Text($0.label).tag($0.name) }
                        }
                        if let help = selected?.help { Text(help).textSelection(.enabled) }
                        LNSFormField(title: "Connection name") { TextField("e.g. work", text: $label) }
                        if nameTaken { Text("That name is already saved. Choose another name.").foregroundStyle(LNSTheme.warning) }
                        Text("Real credentials stay outside the workload. Connecting grants no sandbox access.").font(.caption)
                        Button("Connect") {
                            answers = ConnectAnswers()
                            Task { await session.begin(offer: offer, method: method, label: label.trimmingCharacters(in: .whitespacesAndNewlines)) }
                        }.buttonStyle(.borderedProminent)
                            .disabled(selected == nil || label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || nameTaken)
                    }
                }.disabled(session.busy || !connected).frame(maxWidth: .infinity, alignment: .leading)
                if let error = session.error {
                    Text(error).foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    if session.progress?.polls == true { Button("Check Status") { Task { await session.checkStatus() } }.disabled(session.busy || !connected) }
                }
            }
            HStack {
                if session.busy { ProgressView().controlSize(.small) }
                Spacer()
                Button(session.completed == nil ? "Cancel" : "Done") {
                    Task { await session.cancel(); dismiss() }
                }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(24).frame(width: 540, height: 590).controlSize(.large).textFieldStyle(.roundedBorder).lnsAppearance()
        .onAppear { if method.isEmpty { method = offer.methods.first { $0.offerable && $0.auth_label != nil }?.name ?? "" } }
        .onChange(of: session.ask) { _ in answers = ConnectAnswers() }
        .onChange(of: session.progress) { _ in answers = ConnectAnswers() }
        .onChange(of: session.completed) { value in if let value { answers = ConnectAnswers(); finish(value) } }
        .onChange(of: connected) { value in if !value { Task { await session.cancel() } } }
        .onDisappear { answers = ConnectAnswers(); Task { await session.cancel() } }
    }
    private var nameTaken: Bool { offer.connections.contains { $0.label == label.trimmingCharacters(in: .whitespacesAndNewlines) } }
    private func submit() {
        let values = answers.values; answers = ConnectAnswers()
        Task { await session.answer(values) }
    }
    private func openBrowser() { Task { await session.openBrowser() } }
}
