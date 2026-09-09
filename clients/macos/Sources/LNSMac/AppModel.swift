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
    private let service: ServiceConnection
    private var watching: Task<Void, Never>?

    init() {
        let path = ProcessInfo.processInfo.environment["LNS_SOCKET_PATH"]
            ?? FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent("Library/Application Support/run.lns/service.sock").path
        service = ServiceConnection(path: path)
    }

    func start() {
        guard watching == nil else { return }
        watching = Task {
            var retry: UInt64 = 1
            while !Task.isCancelled {
                do {
                    for try await data in try service.replies(to: .watchApprovals) {
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

    func stop() { watching?.cancel(); watching = nil }

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
        Task {
            do {
                guard case .shuttingDown = try await service.send(.shutdown) else {
                    throw ServiceError(message: "The service did not confirm shutdown.")
                }
                NSApplication.shared.terminate(nil)
            } catch { notice = error.localizedDescription }
        }
    }
}
