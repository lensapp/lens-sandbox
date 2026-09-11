import AppKit
import SwiftUI

@MainActor
struct DesktopCommands: Commands {
    @ObservedObject var model: AppModel
    @ObservedObject private var dashboard: DashboardModel
    @Environment(\.openWindow) private var openWindow

    init(model: AppModel) {
        self.model = model
        dashboard = model.dashboard
    }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Sandbox…") {
                show(.sandboxes)
                dashboard.creatingSandbox = true
            }.keyboardShortcut("n").disabled(!dashboard.canCreateSandbox)
        }
        CommandMenu("Navigate") {
            Button("Sandboxes") { show(.sandboxes) }.keyboardShortcut("4")
            Button("Connectors") { show(.connectors) }.keyboardShortcut("5")
            Button("Registries") { show(.registries) }.keyboardShortcut("6")
            Button("Audit") { show(.audit) }.keyboardShortcut("1")
            Button("Approvals") { show(.approvals) }.keyboardShortcut("2")
            Button("Live Requests", action: model.showApprovals).keyboardShortcut("3")
        }
        CommandGroup(replacing: .appTermination) {
            Button("Quit Interface") { NSApplication.shared.terminate(nil) }.keyboardShortcut("q")
            Button("Stop Service and Quit LNS…", action: model.quitService)
                .disabled(!model.connected || model.stoppingService)
        }
    }

    private func show(_ page: DashboardPage) {
        model.dashboard.page = page
        openWindow(id: "dashboard")
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
}
