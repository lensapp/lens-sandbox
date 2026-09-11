import Foundation
import Combine
import LNSClient

enum DashboardPage: String, CaseIterable, Identifiable {
    case sandboxes = "Sandboxes", connectors = "Connectors", registries = "Registries", audit = "Audit", approvals = "Approvals"
    var id: String { rawValue }
    var symbol: String {
        switch self {
        case .sandboxes: return "shippingbox"
        case .connectors: return "link"
        case .registries: return "externaldrive.connected.to.line.below"
        case .audit: return "list.bullet.rectangle"
        case .approvals: return "checkmark.shield"
        }
    }
}

@MainActor
final class DashboardModel: ObservableObject {
    @Published private(set) var feed = DashboardFeed()
    @Published var page = DashboardPage.sandboxes
    @Published var managementSheet: ManagementSheet?
    @Published var grantRun = ""
    @Published var creatingSandbox = false
    let creation: SandboxCreation?
    let management: ManagementSession
    let registries: RegistrySession
    let registryBrowser: ((String) throws -> AsyncThrowingStream<HelperProcessEvent, Error>)?
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
    private let service: any ServiceClient
    private var watching = false
    private let refreshes: DashboardRefresh
    private var offerTask: Task<Void, Never>?

    init(service: any ServiceClient, launchSandbox: ((SandboxDraft) throws -> AsyncThrowingStream<HelperProcessEvent, Error>)? = nil,
         registryBrowser: ((String) throws -> AsyncThrowingStream<HelperProcessEvent, Error>)? = nil) {
        self.service = service
        creation = launchSandbox.map { SandboxCreation(launch: $0) }
        management = ManagementSession(service: service)
        registries = RegistrySession(service: service)
        self.registryBrowser = registryBrowser
        refreshes = DashboardRefresh { try await service.dashboard() }
        management.onChange = { [weak self] in self?.objectWillChange.send() }
        creation?.onChange = { [weak self] in self?.objectWillChange.send() }
        registries.onChange = { [weak self] in self?.objectWillChange.send() }
    }

    var data: DashboardData { feed.data }
    var connected: Bool { feed.connected }
    var events: [DashboardEvent] { filters.events(in: data) }
    var history: [DashboardApproval] { filters.approvals(in: data) }
    var waiting: [DashboardApproval] { history.filter { $0.entry.waiting } }
    var archived: [DashboardApproval] { history.filter { !$0.entry.waiting } }
    var waitingCount: Int { filters.waitingCount(in: data) }
    var detail: DashboardEvent? { data.events.first { $0.id == selectedEvent } }
    var sandboxName: String {
        if page == .connectors { return "Installed on this Mac" }
        if page == .registries { return "Accounts used to pull sandbox definitions and images" }
        if page == .sandboxes { return "\(currentSandboxes.count) sandboxes" }
        guard let id = filters.sandbox else { return "All sandboxes" }
        return data.sandboxes.first { $0.id == id }?.name ?? id
    }

    var currentSandboxes: [DashboardSandbox] { data.sandboxes.filter(\.controllable) }
    var canCreateSandbox: Bool { connected && creation != nil && creation?.busy == false && !management.busy }

    func manage(_ command: ManagementCommand, reviewing offer: ConnectorOffer? = nil) async -> Bool {
        guard connected else { return false }
        let success = await management.perform(command, reviewing: offer)
        if success { await refresh() }
        return success
    }

    func watch() async {
        guard !watching else { return }
        watching = true
        defer { watching = false; disconnect() }
        var retry: UInt64 = 1
        while !Task.isCancelled {
            do {
                for try await bytes in try service.replies(to: .watchDashboard, once: false, latestOnly: true) {
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
        loading = true
        defer { loading = false }
        let snapshot = try await refreshes.refresh()
        try Task.checkCancellation()
        feed.receive(snapshot)
        management.serviceConnected()
        registries.setConnected(true)
        connectionNotice = nil
        if let id = selectedEvent, !snapshot.events.contains(where: { $0.id == id }) { selectedEvent = nil }
        if let id = selectedHistory, !snapshot.approvals.contains(where: { $0.id == id }) { clearHistory() }
        await management.refresh()
        await registries.refresh()
    }

    private func disconnect() {
        refreshes.cancel()
        feed.disconnect()
        management.disconnect()
        registries.setConnected(false)
        managementSheet = nil
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
