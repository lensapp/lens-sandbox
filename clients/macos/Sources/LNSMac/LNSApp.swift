import AppKit
import SwiftUI
import LNSClient

@main
@MainActor
struct LNSApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Window("LNS", id: "dashboard") {
            DashboardView(model: delegate.model.dashboard, live: delegate.model, mark: (try? delegate.icons.get())?.menuBar)
        }
        .defaultSize(width: 1100, height: 740)
        .windowResizability(.contentMinSize)
        .commands { DesktopCommands(model: delegate.model) }
        MenuBarExtra {
            MenuContent(model: delegate.model)
        } label: {
            if case let .success(icons) = delegate.icons {
                Image(nsImage: icons.menuBar).renderingMode(.template).accessibilityLabel("LNS")
            } else {
                Text("LNS")
            }
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()
    let icons = Result { try AppIcons(bundle: .main) }
    private var panel: ApprovalPanel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        switch icons {
        case let .success(icons): NSApplication.shared.applicationIconImage = icons.dock
        case let .failure(error): model.notice = error.localizedDescription
        }
        let panel = ApprovalPanel(model: model)
        self.panel = panel
        model.onSnapshot = { [weak panel] in panel?.update($0) }
        model.onShowApprovals = { [weak panel] in panel?.show(focus: true) }
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
        Button("Sandboxes…") { dashboard(.sandboxes) }
        Button("Connectors…") { dashboard(.connectors) }
        Button("Registries…") { dashboard(.registries) }
        Button("Audit…") { dashboard(.audit) }
        Button("Approvals…") {
            dashboard(.approvals)
        }
        Button("Live Requests…", action: model.showApprovals)
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
    private var hosting: NSHostingView<ApprovalOverlay>?
    private var anchor: NSRect?

    init(model: AppModel) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 440, height: 289),
                   styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        title = "LNS Access Requests"
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        appearance = NSAppearance(named: .darkAqua)
        let hosting = NSHostingView(rootView: ApprovalOverlay(model: model,
            hide: { [weak self] in self?.orderOut(nil) },
            resize: { [weak self] in self?.resize(to: $0) }))
        self.hosting = hosting
        contentView = hosting
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func cancelOperation(_ sender: Any?) { orderOut(sender) }

    func update(_ snapshot: ApprovalSnapshot) {
        switch presentation.update(snapshot) {
        case .hide: orderOut(nil); return
        case .unchanged: return
        case .show: break
        }
        show()
    }

    func show(focus: Bool = false) {
        let pointer = NSEvent.mouseLocation
        if let screen = NSScreen.screens.first(where: { NSMouseInRect(pointer, $0.frame, false) }) ?? NSScreen.main {
            let bounds = screen.visibleFrame
            anchor = bounds.insetBy(dx: 20, dy: 20)
            hosting?.rootView.maximumHeight = min(620, bounds.height - 40)
            resize(to: min(frame.height, bounds.height - 40))
        }
        orderFrontRegardless()
        if focus { makeKey() }
    }

    private func resize(to height: CGFloat) {
        let top = anchor?.maxY ?? frame.maxY
        let right = anchor?.maxX ?? frame.maxX
        let size = NSSize(width: 440, height: height)
        setFrame(NSRect(x: right - size.width, y: top - size.height, width: size.width, height: size.height), display: true)
    }
}
