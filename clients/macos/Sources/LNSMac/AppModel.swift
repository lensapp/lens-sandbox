import AppKit
import Combine
import LNSClient

@MainActor
final class AppModel: ObservableObject {
    @Published private(set) var feed = ApprovalFeed()
    var snapshot: ApprovalSnapshot { feed.snapshot }
    var connected: Bool { feed.connected }
    @Published private(set) var busy: Set<String> = []
    @Published var notice: String?
    @Published var connectionNotice: String?
    var onSnapshot: ((ApprovalSnapshot) -> Void)?
    var onShowApprovals: (() -> Void)?
    var openDashboard: (() -> Void)?
    @Published var reviewingApprovalID: String?
    private let service: any ServiceClient
    let socketPath: String
    private let control: ServiceControl
    var startingService: Bool { control.phase == .starting }
    var stoppingService: Bool { control.phase == .stopping }
    var canStartService: Bool { control.canStart && !connected }
    let dashboard: DashboardModel
    private var watching: Task<Void, Never>?

    convenience init() {
        let path = ProcessInfo.processInfo.environment["LNS_SOCKET_PATH"]
            ?? FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent("Library/Application Support/run.lns/service.sock").path
        let connection = ServiceConnection(path: path)
        let launch = ServiceLaunch(bundle: Bundle.main.bundleURL, socket: path, environment: ProcessInfo.processInfo.environment)
        let available = FileManager.default.isExecutableFile(atPath: launch.executable.path)
            && FileManager.default.isExecutableFile(atPath: launch.environment["LNS_SERVICE_BIN"] ?? "")
        self.init(service: connection, socketPath: path,
                  launchService: available ? { try await ServiceProcess.start(launch) } : nil,
                  confirmStop: Self.confirmServiceStop,
                  quit: { NSApplication.shared.terminate(nil) },
                  launchSandbox: available ? { draft in
                      HelperProcess.launch(try HelperProcessLaunch(draft: draft, bundle: Bundle.main.bundleURL,
                          socket: path, environment: ProcessInfo.processInfo.environment))
                  } : nil,
                  registryBrowser: available ? { registry in
                      HelperProcess.launch(HelperProcessLaunch(arguments: ["login", registry], bundle: Bundle.main.bundleURL,
                          socket: path, environment: ProcessInfo.processInfo.environment))
                  } : nil)
        if let bytes = UserDefaults.standard.data(forKey: "recentSandboxSources") {
            do {
                let recent = try JSONDecoder().decode(RecentSources.self, from: bytes)
                dashboard.updateRecents { $0 = recent }
            } catch { dashboard.notice = "Could not read recent sources: \(error.localizedDescription)" }
        }
        dashboard.persistRecents = { recent in
            UserDefaults.standard.set(try JSONEncoder().encode(recent), forKey: "recentSandboxSources")
        }
    }

    init(service connection: any ServiceClient, socketPath: String,
         launchService: (() async throws -> Void)?, confirmStop: @escaping () -> Bool,
         quit: @escaping () -> Void,
         launchSandbox: ((SandboxDraft) throws -> AsyncThrowingStream<HelperProcessEvent, Error>)? = nil,
         registryBrowser: ((String) throws -> AsyncThrowingStream<HelperProcessEvent, Error>)? = nil) {
        service = connection
        self.socketPath = socketPath
        control = ServiceControl(client: connection, launch: launchService, confirm: confirmStop, quit: quit)
        dashboard = DashboardModel(service: connection, launchSandbox: launchSandbox, registryBrowser: registryBrowser)
        control.onChange = { [weak self] in self?.objectWillChange.send() }
        control.onError = { [weak self] in self?.notice = $0 }
    }

    func start() {
        guard watching == nil else { return }
        watching = Task {
            await control.startOnLaunch()
            var retry: UInt64 = 1
            while !Task.isCancelled {
                do {
                    for try await data in try service.replies(to: .watchApprovals, once: false, latestOnly: true) {
                        try Task.checkCancellation()
                        guard case let .snapshot(update) = try ServiceReply.decode(data) else {
                            throw ServiceError(message: "The service returned an unexpected approval update.")
                        }
                        connectionNotice = nil
                        retry = 1
                        feed.receive(update)
                        onSnapshot?(update)
                    }
                    if !Task.isCancelled { connectionNotice = "The service disconnected. Reconnecting…" }
                } catch {
                    if !Task.isCancelled { connectionNotice = "Cannot reach LNS: \(error.localizedDescription)" }
                }
                feed.disconnect()
                onSnapshot?(.empty)
                do { try await Task.sleep(nanoseconds: retry * 1_000_000_000) }
                catch { break }
                retry = min(retry * 2, 30)
            }
        }
    }

    func stop() { watching?.cancel(); watching = nil; feed.disconnect(); onSnapshot?(.empty) }

    func respond(to approval: LiveApproval, with action: ApprovalAction) {
        guard connected, !busy.contains(approval.id) else { return }
        busy.insert(approval.id)
        notice = nil
        Task {
            defer { busy.remove(approval.id) }
            do {
                switch try await service.send(.respond(token: approval.token, action: action)) {
                case .submitted: break
                case .stale: notice = "That approval changed or was answered elsewhere. Use the current request."
                default: throw ServiceError(message: "The service did not accept this answer.")
                }
            } catch { notice = error.localizedDescription }
        }
    }

    func quitService() {
        Task { await control.stop(connected: connected) }
    }

    func showApprovals() { onShowApprovals?() }

    func reviewApproval(_ id: String) {
        dashboard.page = .approvals
        dashboard.selectSandbox(nil)
        dashboard.filters.answers = []
        reviewingApprovalID = id
        openDashboard?()
    }

    func startService() {
        guard canStartService else { return }
        notice = nil
        Task { await control.start() }
    }

    private static func confirmServiceStop() -> Bool {
        let alert = NSAlert()
        alert.messageText = "Stop LNS and its running sandboxes?"
        alert.informativeText = "This stops the background service and interrupts workloads. Quit Interface leaves them running."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Stop Service and Quit")
        return alert.runModal() == .alertSecondButtonReturn
    }

    func dismissNotices() {
        let observed = snapshot.notices
        guard connected, !observed.isEmpty else { return }
        Task {
            do {
                for request in try ServiceRequest.noticeDismissalBatches(observed) {
                    guard case .acknowledged = try await service.send(request) else {
                        throw ServiceError(message: "The service did not confirm notice dismissal.")
                    }
                }
            } catch { notice = error.localizedDescription }
        }
    }

}
