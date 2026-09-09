import Foundation
import Combine
import LNSClient

enum DashboardPage: String, CaseIterable, Identifiable {
    case audit = "Audit", approvals = "Approvals"
    var id: String { rawValue }
}

@MainActor
final class DashboardModel: ObservableObject {
    @Published private(set) var feed = DashboardFeed()
    @Published var page = DashboardPage.audit
    @Published var filters = DashboardFilters()
    @Published var selectedEvent: String?
    @Published private(set) var selectedHistory: String?
    @Published private(set) var offer: ConnectorOffer?
    @Published private(set) var offerLoading = false
    @Published private(set) var offerError: String?
    @Published private(set) var loading = false
    @Published private(set) var busy: Set<String> = []
    @Published private(set) var connectionNotice: String?
    @Published var notice: String?
    @Published var archiveChoice: Bool?
    private let service: ServiceConnection
    private var watching = false
    private var refreshTask: Task<DashboardData, Error>?
    private var offerTask: Task<Void, Never>?

    init(service: ServiceConnection) { self.service = service }

    var data: DashboardData { feed.data }
    var connected: Bool { feed.connected }
    var events: [DashboardEvent] { filters.events(in: data) }
    var history: [DashboardApproval] { filters.approvals(in: data) }
    var waiting: [DashboardApproval] { history.filter { $0.entry.waiting } }
    var archived: [DashboardApproval] { history.filter { !$0.entry.waiting } }
    var waitingCount: Int { filters.waitingCount(in: data) }
    var detail: DashboardEvent? { data.events.first { $0.id == selectedEvent } }
    var sandboxName: String {
        guard let id = filters.sandbox else { return "All sandboxes" }
        return data.sandboxes.first { $0.id == id }?.name ?? id
    }

    func watch() async {
        guard !watching else { return }
        watching = true
        defer { watching = false; refreshTask?.cancel(); refreshTask = nil }
        var retry: UInt64 = 1
        while !Task.isCancelled {
            do {
                for try await bytes in try service.replies(to: .watchDashboard) {
                    try Task.checkCancellation()
                    guard case .changed = try JSONDecoder().decode(DashboardMessage.self, from: bytes) else {
                        throw ServiceError(message: "The service returned an unexpected dashboard notification.")
                    }
                    try await Task.sleep(nanoseconds: 150_000_000)
                    try await reload()
                    retry = 1
                }
                if !Task.isCancelled { connectionNotice = "The service disconnected. Reconnecting…" }
            } catch {
                if !Task.isCancelled { connectionNotice = error.localizedDescription }
            }
            disconnect()
            do { try await Task.sleep(nanoseconds: retry * 1_000_000_000) }
            catch { break }
            retry = min(retry * 2, 30)
        }
    }

    func refresh() async {
        do { try await reload() }
        catch { connectionNotice = error.localizedDescription; disconnect() }
    }

    private func reload() async throws {
        if let refreshTask { _ = try await refreshTask.value; return }
        loading = true
        let task = Task { try await service.dashboard() }
        refreshTask = task
        defer { refreshTask = nil; loading = false }
        let snapshot = try await withTaskCancellationHandler {
            try await task.value
        } onCancel: { task.cancel() }
        try Task.checkCancellation()
        feed.receive(snapshot)
        connectionNotice = nil
        if let id = selectedEvent, !snapshot.events.contains(where: { $0.id == id }) { selectedEvent = nil }
        if let id = selectedHistory, !snapshot.approvals.contains(where: { $0.id == id }) { clearHistory() }
    }

    private func disconnect() {
        feed.disconnect()
        selectedEvent = nil
        clearHistory()
    }

    func selectSandbox(_ id: String?) {
        filters.sandbox = id
        selectedEvent = nil
        clearHistory()
    }

    func selectHistory(_ approval: DashboardApproval) {
        let close = selectedHistory == approval.id
        clearHistory()
        guard !close, connected else { return }
        selectedHistory = approval.id
        guard approval.grantable else { return }
        offerLoading = true
        offerTask = Task {
            do {
                let reply = try await service.send(.inspectOffer(id: approval.id))
                try Task.checkCancellation()
                guard selectedHistory == approval.id, connected else { return }
                guard case let .offer(current) = reply else {
                    throw ServiceError(message: "The service did not return the current connector offer.")
                }
                offer = current
                if current == nil { offerError = "This sandbox is not holding that offer; grant it with lns connector grant." }
            } catch {
                if !Task.isCancelled, selectedHistory == approval.id { offerError = error.localizedDescription }
            }
            if !Task.isCancelled, selectedHistory == approval.id { offerLoading = false }
        }
    }

    func clearHistory() {
        offerTask?.cancel(); offerTask = nil
        selectedHistory = nil; offer = nil; offerError = nil; offerLoading = false
    }

    func perform(_ request: ServiceRequest, for id: String) {
        guard connected, !busy.contains(id) else { return }
        busy.insert(id)
        notice = nil
        Task {
            defer { busy.remove(id) }
            do {
                guard case .acknowledged = try await service.send(request) else {
                    throw ServiceError(message: "The service did not acknowledge that action.")
                }
                clearHistory()
                try await reload()
            } catch { notice = error.localizedDescription }
        }
    }
}
