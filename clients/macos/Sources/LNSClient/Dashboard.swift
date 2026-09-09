import Foundation

public struct DashboardSandbox: Decodable, Identifiable, Equatable {
    public let id: String
    public let name: String
    public let image: String
    public let status: String
}

public struct DashboardEvent: Decodable, Identifiable, Equatable {
    public let id: String
    public let ts: String
    public let when: String
    public let run: String
    public let kind: String
    public let detail: String
    public let raw: String
}

public enum HistoryAnswer: String, Codable, CaseIterable, Identifiable {
    case alwaysAllow = "always-allow", alwaysDeny = "always-deny", askAgain = "ask-again"
    public var id: String { rawValue }
    public var label: String {
        switch self {
        case .alwaysAllow: return "Always allow"
        case .alwaysDeny: return "Always deny"
        case .askAgain: return "Ask again"
        }
    }
}

public struct ApprovalHistoryEntry: Decodable, Identifiable, Equatable {
    public let id: String
    public let sandbox: String?
    public let subject: String
    public let action: String?
    public let kind: String
    public let answer: String
    public let answerable: Bool
    public var waiting: Bool { kind != "notice" && ["undecided", "withdrawn"].contains(answer) }
}

public struct DashboardApproval: Decodable, Identifiable, Equatable {
    public let entry: ApprovalHistoryEntry
    public let raw: Bool
    public let answers: [HistoryAnswer]
    public let grantable: Bool
    public var id: String { entry.id }
}

public struct DashboardData: Equatable {
    public var sandboxes: [DashboardSandbox] = []
    public var events: [DashboardEvent] = []
    public var approvals: [DashboardApproval] = []
    public var warnings: [String] = []
    public init() {}
}

public enum DashboardMessage: Decodable {
    case begin, end, changed
    case sandbox(DashboardSandbox), event(DashboardEvent), approval(DashboardApproval), warning(String)

    private enum CodingKeys: String, CodingKey { case type, sandbox, event, approval, message }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .type) {
        case "DashboardBegin": self = .begin
        case "DashboardEnd": self = .end
        case "DashboardChanged": self = .changed
        case "DashboardSandbox": self = .sandbox(try values.decode(DashboardSandbox.self, forKey: .sandbox))
        case "DashboardEvent": self = .event(try values.decode(DashboardEvent.self, forKey: .event))
        case "DashboardApproval": self = .approval(try values.decode(DashboardApproval.self, forKey: .approval))
        case "DashboardWarning": self = .warning(try values.decode(String.self, forKey: .message))
        case "Error": throw ServiceError(message: try values.decode(String.self, forKey: .message))
        default: throw ServiceError(message: "The service returned an unexpected dashboard response.")
        }
    }
}

public struct DashboardRead {
    private var pending = DashboardData()
    private var begun = false
    private var complete = false
    public init() {}
    public mutating func receive(_ message: DashboardMessage) throws -> DashboardData? {
        guard !complete else { throw ServiceError(message: "The dashboard sent data after completion.") }
        if case .begin = message {
            guard !begun else { throw ServiceError(message: "The dashboard restarted before completing.") }
            begun = true
            return nil
        }
        guard begun else { throw ServiceError(message: "The dashboard did not start a snapshot.") }
        switch message {
        case let .sandbox(sandbox): pending.sandboxes.append(sandbox)
        case let .event(event): pending.events.append(event)
        case let .approval(approval): pending.approvals.append(approval)
        case let .warning(warning): pending.warnings.append(warning)
        case .end: complete = true; return pending
        case .begin, .changed: throw ServiceError(message: "The dashboard returned an unexpected message.")
        }
        return nil
    }
    public func finish() throws {
        guard complete else { throw ServiceError(message: "The dashboard disconnected before its snapshot was complete. Reconnect or refresh to try again.") }
    }
}

public struct DashboardFeed {
    public private(set) var data = DashboardData()
    public private(set) var connected = false
    public init() {}
    public mutating func receive(_ data: DashboardData) { self.data = data; connected = true }
    public mutating func disconnect() { data = DashboardData(); connected = false }
}

public struct DashboardFilters {
    public var sandbox: String?
    public var kinds: Set<String> = []
    public var answers: Set<String> = []
    public var search = ""
    public init() {}

    public func events(in data: DashboardData) -> [DashboardEvent] {
        let needle = search.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return data.events.filter { event in
            if !needle.isEmpty {
                return [event.when, event.run, event.kind, event.detail].contains { $0.lowercased().contains(needle) }
            }
            return (sandbox == nil || event.run == sandbox)
                && (kinds.isEmpty || kinds.contains(event.kind))
        }
    }
    public func approvals(in data: DashboardData, applyAnswers: Bool = true) -> [DashboardApproval] {
        let name = data.sandboxes.first { $0.id == sandbox }?.name ?? sandbox
        return data.approvals.filter { approval in
            (name == nil || approval.entry.sandbox == name)
                && (!applyAnswers || answers.isEmpty || answers.contains(approval.entry.answer))
        }
    }
    public func waitingCount(in data: DashboardData) -> Int {
        approvals(in: data, applyAnswers: false).filter { $0.entry.waiting }.count
    }
}
