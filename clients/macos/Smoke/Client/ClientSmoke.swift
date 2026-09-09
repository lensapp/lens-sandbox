import Foundation
import LNSClient

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ServiceError(message: message) }
}

@main
struct ClientSmoke {
    static func main() async throws {
        if CommandLine.arguments.count == 3 && CommandLine.arguments[1] == "--probe" {
            let client = ServiceConnection(path: CommandLine.arguments[2])
            guard case .offer(nil) = try await client.send(.inspectOffer(id: "gone")) else {
                throw ServiceError(message: "local transport probe returned an unexpected reply")
            }
            print("PASS: local transport")
            return
        }
        try require(CommandLine.arguments.count == 3, "usage: LNSClientSmoke <socket> <entry-id>")
        let client = ServiceConnection(path: CommandLine.arguments[1])
        let id = CommandLine.arguments[2]
        var updates = try client.replies(to: .watchDashboard).makeAsyncIterator()
        let first = try await updates.next()
        try require(first != nil, "missing initial dashboard notification")
        if let first {
            guard case .changed = try JSONDecoder().decode(DashboardMessage.self, from: first) else {
                throw ServiceError(message: "unexpected initial dashboard notification")
            }
        }
        let snapshot = try await client.dashboard()
        try require(snapshot.sandboxes.contains { $0.name == "quiet_river" }, "historical sandbox missing")
        try require(snapshot.events.count == 1, "audit event missing")
        try require(snapshot.warnings.contains { $0.contains("anchor") }, "integrity warning missing")
        try require(snapshot.approvals.contains { $0.id == id }, "approval history missing")
        guard case .acknowledged = try await client.send(.answerHistory(id: id, answer: .alwaysAllow)) else {
            throw ServiceError(message: "answer not acknowledged")
        }
        try require(try await updates.next() != nil, "answer did not notify subscribers")
        let answered = try await client.dashboard()
        try require(answered.approvals.contains { $0.id == id && $0.entry.answer == "always allow" }, "answer not persisted")
        guard case .acknowledged = try await client.send(.removeHistory(id: id)) else {
            throw ServiceError(message: "removal not acknowledged")
        }
        let removed = try await client.dashboard()
        try require(removed.approvals.isEmpty, "removed history still listed")
        guard case .offer(nil) = try await client.send(.inspectOffer(id: "gone")) else {
            throw ServiceError(message: "missing connector offer was not explicit")
        }
        var live = try client.replies(to: .watchApprovals).makeAsyncIterator()
        guard let bytes = try await live.next(), case let .snapshot(approvals) = try ServiceReply.decode(bytes) else {
            throw ServiceError(message: "live approval subscription failed")
        }
        try require(approvals.approvals.isEmpty, "unexpected live request")
        guard case .shuttingDown = try await client.send(.shutdown) else {
            throw ServiceError(message: "shutdown not acknowledged")
        }
        while try await updates.next() != nil {}
        try require(try await live.next() == nil, "shutdown did not close approval subscription")
        print("PASS: Swift client and real service agree on audit, history, actions, notifications, and shutdown")
    }
}
