import SwiftUI
import LNSClient

@MainActor
struct RegistryList: View {
    @ObservedObject var model: DashboardModel
    @State private var showingLogin = false
    @State private var signingOut: RegistryLogin?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Registries").font(.title2.weight(.semibold))
                Spacer()
                Button("Sign In…") { showingLogin = true }
                    .buttonStyle(.borderedProminent).disabled(!model.registries.connected || model.registries.busy)
            }
            Text("Sign in to the registries hosting your sandbox definitions and base images. These accounts are shared with the CLI through the LNS service.")
                .foregroundStyle(.secondary)
            if let error = model.registries.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).textSelection(.enabled)
            }
            List(model.registries.logins) { login in
                HStack {
                    Image(systemName: "externaldrive.connected.to.line.below").font(.title2).foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(login.registry).font(.headline)
                        Text(login.username).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Sign Out…", role: .destructive) { signingOut = login }
                        .disabled(!model.registries.connected || model.registries.busy)
                }.padding(.vertical, 8)
            }
            .overlay {
                if model.registries.logins.isEmpty {
                    Text(model.registries.connected ? "No registry accounts saved. Sign in to pull images that require authentication." : "Waiting for the service…")
                        .foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 340)
                }
            }
        }
        .padding(20)
        .sheet(isPresented: $showingLogin) { RegistryLoginForm(model: model) }
        .confirmationDialog("Sign out of \(signingOut?.registry ?? "this registry")?", isPresented: Binding(
            get: { signingOut != nil }, set: { if !$0 { signingOut = nil } }), titleVisibility: .visible) {
                if let login = signingOut {
                    Button("Sign Out", role: .destructive) { Task { _ = await model.registries.logout(login.registry) } }
                }
        } message: { Text("Future pulls from this registry may require signing in again.") }
    }
}

@MainActor
struct RegistryLoginForm: View {
    @ObservedObject var model: DashboardModel
    @Environment(\.dismiss) private var dismiss
    @State private var registry = "hub.lns.run"
    @State private var username = ""
    @State private var secret = ""
    @State private var browser = true
    @State private var attempted = false

    private var session: RegistrySession { model.registries }
    private var valid: Bool {
        (try? RegistrySession.host(registry)) != nil && (browser ? model.registryBrowser != nil : !username.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !secret.isEmpty)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Sign In to a Registry").font(.title2.weight(.semibold))
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 14) {
                        TextField("Registry host, e.g. hub.lns.run or ghcr.io", text: $registry).textFieldStyle(.roundedBorder)
                        if let hostError { Text(hostError).font(.caption).foregroundStyle(.orange) }
                        Picker("Sign-in option", selection: $browser) {
                            Text("Browser").tag(true).disabled(model.registryBrowser == nil)
                            Text("Username and Token").tag(false)
                        }.pickerStyle(.segmented)
                        if browser {
                            Text("Your browser opens to approve sign-in. The confirmation code appears below. Registries without browser login can use a username and token.")
                                .foregroundStyle(.secondary)
                        } else {
                            TextField("Username", text: $username).textFieldStyle(.roundedBorder)
                            SecureField("Password or access token", text: $secret).textFieldStyle(.roundedBorder)
                            Text("LNS verifies these credentials before saving them outside the sandbox.").font(.caption).foregroundStyle(.secondary)
                        }
                        Text("Signing in replaces any saved account for this registry.").font(.caption).foregroundStyle(.secondary)
                    }.disabled(session.busy)
                    if attempted, let error = session.error {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).textSelection(.enabled)
                    }
                    if attempted, !session.output.isEmpty {
                        Text(session.output).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            HStack {
                if session.busy { ProgressView().controlSize(.small); Text(browser ? "Waiting for browser sign-in…" : "Verifying credentials…").font(.caption) }
                Spacer()
                Button("Cancel") { session.cancelBrowser(); dismiss() }.keyboardShortcut(.cancelAction)
                    .disabled(session.busy && !browser)
                Button(browser ? "Sign In with Browser" : "Sign In", action: submit)
                    .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
                    .disabled(!valid || !session.connected || session.busy)
            }
        }
        .padding(24).frame(width: 580, height: 470)
        .interactiveDismissDisabled(session.busy && !browser)
        .onAppear { if model.registryBrowser == nil { browser = false } }
        .onChange(of: browser) { _ in secret = "" }
        .onDisappear { secret = ""; if browser { session.cancelBrowser() } }
    }

    private var hostError: String? {
        guard !registry.isEmpty else { return nil }
        do { _ = try RegistrySession.host(registry); return nil }
        catch { return error.localizedDescription }
    }

    private func submit() {
        guard valid, !session.busy else { return }
        attempted = true
        let host = registry
        let account = username.trimmingCharacters(in: .whitespacesAndNewlines)
        let credential = secret
        secret = ""
        Task {
            let success: Bool
            if browser, let launch = model.registryBrowser {
                success = await session.loginInBrowser(host, launch: launch)
            } else { success = await session.login(host, username: account, secret: credential) }
            if success { dismiss() }
        }
    }
}
