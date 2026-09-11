import SwiftUI
import LNSClient

@MainActor
struct RegistryList: View {
    @ObservedObject var model: DashboardModel
    @State private var showingLogin = false
    @State private var signingOut: RegistryLogin?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            LNSPageHeader(title: "Registries", subtitle: "Manage accounts for private definitions and images.") {
                Button("Sign In…") { showingLogin = true }
                    .buttonStyle(.borderedProminent).disabled(!model.registries.connected || model.registries.busy)
            }
            Label("Credentials stay on this Mac and are shared with the CLI.", systemImage: "lock.shield")
                .font(.system(size: 12)).foregroundStyle(LNSTheme.muted)
                .padding(.horizontal, 24).padding(.bottom, 16)
            if let error = model.registries.error {
                Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    .padding(.horizontal, 24).padding(.bottom, 16)
            }
            List(model.registries.logins) { login in
                HStack(spacing: 16) {
                    Image(systemName: "externaldrive.connected.to.line.below").font(.system(size: 20)).foregroundStyle(LNSTheme.accent)
                        .frame(width: 36, height: 36)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(login.registry).font(.system(size: 13, weight: .semibold, design: .monospaced)).foregroundStyle(LNSTheme.heading)
                            .lineLimit(1).help(login.registry)
                        Text(login.username).font(.caption).foregroundStyle(LNSTheme.muted).lineLimit(1).help(login.username)
                    }
                    Spacer()
                    Button("Sign Out…", role: .destructive) { signingOut = login }
                        .disabled(!model.registries.connected || model.registries.busy)
                }.padding(.vertical, 12).listRowBackground(LNSTheme.surface)
            }
            .listStyle(.inset).scrollContentBackground(.hidden).lnsPanel()
            .overlay {
                if model.registries.logins.isEmpty {
                    LNSEmptyState(
                        symbol: "externaldrive.connected.to.line.below",
                        title: model.registries.connected ? "No registry accounts saved" : "Waiting for the service…",
                        message: "Sign in to pull definitions and images that require authentication."
                    )
                }
            }
            .padding(.horizontal, 24).padding(.bottom, 24)
        }
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
            LNSPageHeading(title: "Sign In to a Registry", subtitle: "Use your browser or registry credentials.")
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 14) {
                        LNSFormField(title: "Registry") {
                            TextField("hub.lns.run or ghcr.io", text: $registry).textFieldStyle(.roundedBorder)
                        }
                        if let hostError { Text(hostError).font(.caption).foregroundStyle(LNSTheme.warning) }
                        Picker("Sign-in option", selection: $browser) {
                            Text("Browser").tag(true).disabled(model.registryBrowser == nil)
                            Text("Username and Token").tag(false)
                        }.pickerStyle(.segmented)
                        if browser {
                            Text("Your browser opens to approve sign-in. The confirmation code appears below. Registries without browser login can use a username and token.")
                                .foregroundStyle(LNSTheme.muted)
                        } else {
                            LNSFormField(title: "Username") {
                                TextField("Username", text: $username).textFieldStyle(.roundedBorder)
                            }
                            LNSFormField(title: "Password or access token") {
                                SecureField("Password or access token", text: $secret).textFieldStyle(.roundedBorder)
                            }
                            Text("LNS verifies these credentials before saving them outside the sandbox.").font(.caption).foregroundStyle(LNSTheme.muted)
                        }
                        Text("Signing in replaces any saved account for this registry.").font(.caption).foregroundStyle(LNSTheme.muted)
                    }.disabled(session.busy)
                    if attempted, let error = session.error {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(LNSTheme.warning).textSelection(.enabled)
                    }
                    if attempted, !session.output.isEmpty {
                        Text(session.output).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(12).lnsPanel()
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
        .padding(24).frame(width: 580, height: 540)
        .controlSize(.large)
        .lnsAppearance()
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
