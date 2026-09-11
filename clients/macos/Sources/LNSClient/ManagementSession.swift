import Foundation

private struct ManagementEnvelope: Decodable { let type: String }

extension ServiceClient {
    public func manage(_ command: ManagementCommand) async throws -> ServiceReply {
        let expected: String
        switch command.type {
        case "StartRun": expected = "RunStarted"
        case "StopRun": expected = "RunStopped"
        case "RemoveRun": expected = "Acknowledged"
        case "ListConnectors": expected = "ConnectorList"
        case "InstallConnector": expected = "ConnectorInstalled"
        case "UninstallConnector": expected = "ConnectorUninstalled"
        case "ConnectConnector": expected = "ConnectorConnected"
        case "DisconnectConnector": expected = "ConnectorDisconnected"
        case "GrantConnector": expected = "ConnectorGranted"
        case "ForgetConnector": expected = "ConnectorForgotten"
        default: throw ServiceError(message: "Unknown management action.")
        }
        for try await bytes in try replies(to: .management(command), once: false, latestOnly: false) {
            try Task.checkCancellation()
            let type = try JSONDecoder().decode(ManagementEnvelope.self, from: bytes).type
            if type == "RunProgress" || type == "RunLog" { continue }
            let reply = try ServiceReply.decode(bytes)
            guard type == expected else { throw ServiceError(message: "The service did not confirm that action. Refresh before trying again.") }
            return reply
        }
        throw ServiceError(message: "The service disconnected before confirming the action. Refresh before trying again.")
    }
}

@MainActor
public final class ManagementSession {
    public private(set) var connectors: [ConnectorOffer] = []
    public private(set) var connected = false
    public private(set) var loading = false
    public private(set) var busy = false
    public private(set) var error: String?
    public private(set) var message: String?
    public var onChange: (() -> Void)?
    private let service: any ServiceClient
    private var generation = 0
    private var refreshTask: Task<[ConnectorOffer], Error>?
    private var dirty = false

    public init(service: any ServiceClient) { self.service = service }
    public func serviceConnected() { connected = true; onChange?() }
    public func dismissMessage() { message = nil; onChange?() }
    public func refresh() async {
        let current = generation
        let task: Task<[ConnectorOffer], Error>
        if let pending = refreshTask {
            dirty = true
            task = pending
        } else {
            loading = true; onChange?()
            task = Task {
                defer { if generation == current { refreshTask = nil } }
                while true {
                    dirty = false
                    let inventory = try await readInventory()
                    try Task.checkCancellation()
                    if !dirty { return inventory }
                }
            }
            refreshTask = task
        }
        do {
            let inventory = try await task.value
            try Task.checkCancellation()
            guard current == generation else { return }
            connectors = inventory; connected = true; error = nil
        } catch {
            guard current == generation else { return }
            connectors = []
            self.error = error.localizedDescription
        }
        loading = false; onChange?()
    }

    public func disconnect() {
        generation += 1
        refreshTask?.cancel(); refreshTask = nil; dirty = false
        connectors = []; connected = false; loading = false; error = nil; message = nil
        onChange?()
    }

    public func perform(_ command: ManagementCommand, reviewing offer: ConnectorOffer? = nil) async -> Bool {
        guard connected, !busy else { return false }
        let current = generation
        busy = true; error = nil; message = nil; onChange?()
        defer { busy = false; onChange?() }
        do {
            if let offer {
                let inventory = try await readInventory()
                guard current == generation, connected else { return false }
                guard inventory.first(where: { $0.name == offer.name }) == offer else {
                    connectors = inventory
                    throw ServiceError(message: "This connector changed. Close this form and review its current details before continuing.")
                }
            }
            try Task.checkCancellation()
            guard current == generation, connected else { return false }
            let reply = try await service.manage(command)
            guard current == generation else { return false }
            switch reply {
            case let .completed(outcome): message = outcome
            case .acknowledged: message = "Sandbox removed."
            default: throw ServiceError(message: "The service did not confirm the action.")
            }
            await refresh()
            return true
        } catch {
            if current == generation { self.error = error.localizedDescription }
            return false
        }
    }

    private func readInventory() async throws -> [ConnectorOffer] {
        guard case let .connectors(inventory) = try await service.manage(.listConnectors) else {
            throw ServiceError(message: "The service did not return its connectors.")
        }
        return inventory
    }
}
