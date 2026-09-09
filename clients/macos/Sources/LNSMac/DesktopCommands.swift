import AppKit
import SwiftUI

@MainActor
struct DesktopCommands: Commands {
    @ObservedObject var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandMenu("Navigate") {
            Button("Audit") { show(.audit) }.keyboardShortcut("1")
            Button("Approvals") { show(.approvals) }.keyboardShortcut("2")
            Button("Live Requests") {
                openWindow(id: "approvals")
                NSApplication.shared.activate(ignoringOtherApps: true)
            }.keyboardShortcut("3")
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
