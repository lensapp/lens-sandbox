import AppKit
import SwiftUI
import LNSClient

@main
struct LNSApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
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
}

struct MenuContent: View {
    @ObservedObject var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text(model.connected ? "\(model.snapshot.approvals.count) pending approvals" : "Service disconnected")
        Button("Approvals…") {
            openWindow(id: "approvals")
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
        Divider()
        Button("Stop Service and Quit LNS") { model.quitService() }
            .disabled(!model.connected)
        Button("Quit Interface") { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }
}

@MainActor
final class ApprovalPanel: NSPanel {
    private var presented: Set<String> = []

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
        let current = Set(snapshot.approvals.map(\.id))
        defer { presented = current }
        if current.isEmpty { orderOut(nil); return }
        guard !current.subtracting(presented).isEmpty else { return }
        let pointer = NSEvent.mouseLocation
        if let screen = NSScreen.screens.first(where: { NSMouseInRect(pointer, $0.frame, false) }) ?? NSScreen.main {
            let bounds = screen.visibleFrame
            setFrameTopLeftPoint(NSPoint(x: bounds.maxX - frame.width - 20, y: bounds.maxY - 20))
        }
        orderFrontRegardless()
    }
}
