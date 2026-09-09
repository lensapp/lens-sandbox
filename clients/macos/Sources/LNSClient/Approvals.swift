import Foundation

public struct ApprovalSnapshot: Decodable, Equatable {
    public let approvals: [LiveApproval]
    public let notices: [String]

    public static let empty = ApprovalSnapshot(approvals: [], notices: [])
}

public struct LiveApproval: Decodable, Identifiable, Equatable {
    public let id: String
    public let token: String
    public let host: String
    public let action: String
    public let run: String?
    public let raw: Bool
    public let waiting: Bool
    public let submitting: Bool
    public let offer: ConnectorOffer?
}

public struct ConnectorOffer: Decodable, Equatable {
    public let name: String
    public let digest: String
    public let serves: [String]
    public let methods: [ConnectorMethod]
    public let connections: [ConnectorConnection]
}

public struct ConnectorMethod: Decodable, Identifiable, Equatable {
    public let name: String
    public let label: String
    public let auth_label: String?
    public let offerable: Bool
    public let opens: [String]
    public let writes: [String]
    public let env: [String]
    public let credentials: [String]
    public let asks: [String]
    public let help: String?
    public let overrides: [String]?
    public var id: String { name }
}

public struct ConnectorConnection: Decodable, Identifiable, Equatable {
    public let label: String
    public let method: String
    public let authority: [String]
    public var id: String { label }
}

public enum ApprovalAction: Encodable {
    case allowOnce, allowAlways, denyOnce, denyAlways, dismiss, decline
    case grant(method: String, connection: ConnectionChoice)

    private enum CodingKeys: String, CodingKey { case kind, method, connection }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        let kind: String
        switch self {
        case .allowOnce: kind = "allow_once"
        case .allowAlways: kind = "allow_always"
        case .denyOnce: kind = "deny_once"
        case .denyAlways: kind = "deny_always"
        case .dismiss: kind = "dismiss"
        case .decline: kind = "decline"
        case let .grant(method, connection):
            kind = "grant"
            try container.encode(method, forKey: .method)
            try container.encode(connection, forKey: .connection)
        }
        try container.encode(kind, forKey: .kind)
    }
}

public enum ConnectionChoice: Encodable {
    case none
    case held(label: String)
    case new(label: String, values: [String: String])

    private enum CodingKeys: String, CodingKey { case kind, label, values }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .none:
            try container.encode("none", forKey: .kind)
        case let .held(label):
            try container.encode("held", forKey: .kind)
            try container.encode(label, forKey: .label)
        case let .new(label, values):
            try container.encode("new", forKey: .kind)
            try container.encode(label, forKey: .label)
            try container.encode(values, forKey: .values)
        }
    }
}

public struct ServiceRequest: Encodable {
    public let type: String
    public let token: String?
    public let action: ApprovalAction?

    public static let watchApprovals = ServiceRequest(type: "WatchApprovals", token: nil, action: nil)
    public static let shutdown = ServiceRequest(type: "Shutdown", token: nil, action: nil)

    public static func respond(token: String, action: ApprovalAction) -> Self {
        Self(type: "RespondToApproval", token: token, action: action)
    }
}

public enum ServiceReply {
    case snapshot(ApprovalSnapshot), submitted, stale, shuttingDown

    public static func decode(_ data: Data) throws -> Self {
        struct Envelope: Decodable { let type: String; let message: String? }
        let decoder = JSONDecoder()
        let envelope = try decoder.decode(Envelope.self, from: data)
        switch envelope.type {
        case "LiveApprovals": return .snapshot(try decoder.decode(ApprovalSnapshot.self, from: data))
        case "LiveApprovalSubmitted": return .submitted
        case "LiveApprovalStale": return .stale
        case "ShuttingDown": return .shuttingDown
        case "Error": throw ServiceError(message: envelope.message ?? "The service could not complete the request.")
        default: throw ServiceError(message: "This app and service use different protocols. Update them together.")
        }
    }
}

public struct ServiceError: LocalizedError {
    public let message: String
    public var errorDescription: String? { message }

    public init(message: String) { self.message = message }
}
