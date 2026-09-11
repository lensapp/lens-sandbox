import Foundation

public struct ManagementCommand: Encodable {
    public let type: String
    public private(set) var run: String?
    public private(set) var name: String?
    public private(set) var method: String?
    public private(set) var connection: String?
    public private(set) var source: String?
    public private(set) var values: [String: String]?
    public private(set) var answered_by: String?
    public private(set) var attach: Bool?
    public private(set) var stdin: Bool?
    public private(set) var timeout_secs: UInt64?
    public private(set) var force: Bool?

    private init(type: String) { self.type = type }
    public static func start(_ run: String) -> Self {
        var command = Self(type: "StartRun")
        command.run = run; command.attach = false; command.stdin = false
        return command
    }
    public static func stop(_ run: String) -> Self {
        var command = Self(type: "StopRun")
        command.run = run; command.timeout_secs = 10
        return command
    }
    public static func remove(_ run: String) -> Self {
        var command = Self(type: "RemoveRun")
        command.run = run; command.force = false
        return command
    }
    public static let listConnectors = Self(type: "ListConnectors")
    public static func install(_ source: String) -> Self {
        var command = Self(type: "InstallConnector"); command.source = source
        return command
    }
    public static func uninstall(_ name: String) -> Self {
        var command = Self(type: "UninstallConnector"); command.name = name
        return command
    }
    public static func connect(_ name: String, method: String, label: String, values: [String: String]) -> Self {
        var command = Self(type: "ConnectConnector")
        command.name = name; command.method = method; command.connection = label; command.values = values
        return command
    }
    public static func disconnect(_ name: String, connection: String) -> Self {
        var command = Self(type: "DisconnectConnector")
        command.name = name; command.connection = connection
        return command
    }
    public static func grant(_ name: String, run: String, method: String, connection: String?) -> Self {
        var command = Self(type: "GrantConnector")
        command.name = name; command.run = run; command.method = method; command.connection = connection; command.answered_by = "card"
        return command
    }
    public static func forget(_ name: String, run: String) -> Self {
        var command = Self(type: "ForgetConnector"); command.name = name; command.run = run
        return command
    }
}

public struct GrantSelection {
    public var run = ""
    public var method = ""
    public var connection = ""
    public init() {}
    public func command(offer: ConnectorOffer, sandboxes: [DashboardSandbox]) -> ManagementCommand? {
        guard sandboxes.contains(where: { $0.id == run && $0.controllable }),
              let selected = offer.methods.first(where: { $0.name == method && $0.offerable }) else { return nil }
        if selected.auth_label != nil {
            guard offer.connections.contains(where: { $0.label == connection && $0.method == method }) else { return nil }
        }
        return .grant(offer.name, run: run, method: method, connection: selected.auth_label == nil ? nil : connection)
    }
}

extension DashboardSandbox {
    public var controllable: Bool { status == "running" || status == "exited" }
    public var statusLabel: String { status == "exited" ? "Stopped" : status.capitalized }
}

extension ServiceReply {
    static func management(_ data: Data, type: String) throws -> Self? {
        let decoder = JSONDecoder()
        switch type {
        case "ConnectorList":
            struct Inventory: Decodable { let connectors: [ConnectorOffer] }
            return .connectors(try decoder.decode(Inventory.self, from: data).connectors)
        case "RunStarted":
            struct Started: Decodable { let run_id: String }
            _ = try decoder.decode(Started.self, from: data)
            return .completed("Sandbox started.")
        case "RunStopped":
            struct Stopped: Decodable { let forced: Bool }
            return .completed(try decoder.decode(Stopped.self, from: data).forced ? "Sandbox stopped after forcing it to exit." : "Sandbox stopped.")
        case "ConnectorGranted":
            struct Granted: Decodable { let name: String; let method: String; let unchanged: Bool; let reserved: Bool; let displaced: String? }
            let grant = try decoder.decode(Granted.self, from: data)
            if grant.unchanged { return .completed("This sandbox already has that grant.") }
            let message = grant.reserved ? "Access reserved for a future sandbox." : "Access granted."
            return .completed(message + (grant.displaced.map { " Replaced \($0)." } ?? ""))
        case "ConnectorForgotten":
            struct Forgotten: Decodable { let name: String; let had_decision: Bool; let reserved: Bool }
            let result = try decoder.decode(Forgotten.self, from: data)
            return .completed(result.had_decision ? "Decision forgotten. The sandbox will ask again on its next start." : "This sandbox had no decision to forget.")
        case "ConnectorInstalled":
            struct Installed: Decodable { let connector: ConnectorOffer }
            let result = try decoder.decode(Installed.self, from: data)
            return .completed("\(result.connector.name) installed. Grant access to a sandbox when needed.")
        case "ConnectorConnected":
            struct Connected: Decodable { let name: String; let connection: String; let invalidated: [String] }
            let result = try decoder.decode(Connected.self, from: data)
            let invalidated = result.invalidated.isEmpty ? "" : " These sandboxes need a new grant: \(result.invalidated.joined(separator: ", "))."
            return .completed("Connected as \(result.connection)." + invalidated)
        case "ConnectorDisconnected":
            struct Disconnected: Decodable { let name: String; let dropped: Int }
            let result = try decoder.decode(Disconnected.self, from: data)
            return .completed("Disconnected \(result.dropped) connection(s). Existing sandbox grants remain.")
        case "ConnectorUninstalled":
            struct Uninstalled: Decodable { let name: String; let dropped_connections: Int }
            let result = try decoder.decode(Uninstalled.self, from: data)
            return .completed("\(result.name) uninstalled; \(result.dropped_connections) connection(s) removed. Existing sandbox grants remain.")
        case "RunUnknown", "ConnectorUnknown":
            throw ServiceError(message: "That sandbox or connector is no longer available. Refresh to see the current state.")
        default: return nil
        }
    }
}
