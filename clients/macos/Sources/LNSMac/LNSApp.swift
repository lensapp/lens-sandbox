import AppKit
import SwiftUI
import LNSClient

@main
@MainActor
struct LNSApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Window("LNS", id: "dashboard") {
            DashboardView(model: delegate.model.dashboard, live: delegate.model)
        }
        .defaultSize(width: 1100, height: 740)
        .windowResizability(.contentMinSize)
        .commands { DesktopCommands(model: delegate.model) }
        Window("LNS Approvals", id: "approvals") {
            ApprovalList(model: delegate.model)
                .frame(minWidth: 440, minHeight: 320)
        }
        .defaultSize(width: 520, height: 620)
        MenuBarExtra("LNS", systemImage: "shield.lefthalf.filled") {
            MenuContent(model: delegate.model)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()
    private var panel: ApprovalPanel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let panel = ApprovalPanel(model: model)
        self.panel = panel
        model.onSnapshot = { [weak panel] in panel?.update($0) }
        model.start()
    }

    func applicationWillTerminate(_ notification: Notification) { model.stop() }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}

@MainActor
struct MenuContent: View {
    @ObservedObject var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text(model.connected ? "\(model.snapshot.approvals.count) pending approvals" : "Service disconnected")
        Button("Audit…") { dashboard(.audit) }
        Button("Approvals…") {
            dashboard(.approvals)
        }
        Button("Live Requests…") {
            openWindow(id: "approvals")
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
        if !model.connected {
            Button(model.startingService ? "Starting Service…" : "Start Service", action: model.startService)
                .disabled(!model.canStartService)
        }
        Divider()
        Button("Stop Service and Quit LNS…") { model.quitService() }
            .disabled(!model.connected || model.stoppingService)
        Button("Quit Interface") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }

    private func dashboard(_ page: DashboardPage) {
        model.dashboard.page = page
        openWindow(id: "dashboard")
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
}

@MainActor
final class ApprovalPanel: NSPanel {
    private var presentation = ApprovalPresentation()

    init(model: AppModel) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 440, height: 560),
                   styleMask: [.titled, .closable, .resizable, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        title = "LNS Approval"
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = false
        contentView = NSHostingView(rootView: ApprovalList(model: model))
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    func update(_ snapshot: ApprovalSnapshot) {
        switch presentation.update(snapshot) {
        case .hide: orderOut(nil); return
        case .unchanged: return
        case .show: break
        }
        let pointer = NSEvent.mouseLocation
        if let screen = NSScreen.screens.first(where: { NSMouseInRect(pointer, $0.frame, false) }) ?? NSScreen.main {
            let bounds = screen.visibleFrame
            setFrameTopLeftPoint(NSPoint(x: bounds.maxX - frame.width - 20, y: bounds.maxY - 20))
        }
        orderFrontRegardless()
    }
}
