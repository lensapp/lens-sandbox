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
            var updates = try client.replies(to: .watchDashboard).makeAsyncIterator()
            try require(try await updates.next() != nil, "local transport subscription did not start")
            guard case .offer(nil) = try await client.send(.inspectOffer(id: "gone")) else {
                throw ServiceError(message: "local transport probe returned an unexpected reply")
            }
            for _ in 0..<20 {
                let snapshot = try await client.dashboard()
                try require(snapshot.warnings == ["local transport fixture"], "finite local read lost its data")
            }
            print("PASS: local transport")
            return
        }
        var stage = "initial dashboard subscription"
        defer {
            if stage != "complete" {
                FileHandle.standardError.write(Data("SMOKE FAILURE STAGE: \(stage)\n".utf8))
            }
        }
        try require(CommandLine.arguments.count == 4, "usage: LNSClientSmoke <socket> <entry-id> <connector-path>")
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
        stage = "initial dashboard read"
        let snapshot = try await client.dashboard()
        try require(snapshot.sandboxes.contains { $0.name == "quiet_river" }, "historical sandbox missing")
        try require(snapshot.events.count == 1, "audit event missing")
        try require(snapshot.warnings.contains { $0.contains("too large") }, "oversized audit row was not reported")
        try require(snapshot.warnings.contains { $0.contains("anchor") }, "integrity warning missing")
        try require(snapshot.approvals.contains { $0.id == id }, "approval history missing")
        stage = "answer history"
        guard case .acknowledged = try await client.send(.answerHistory(id: id, answer: .alwaysAllow)) else {
            throw ServiceError(message: "answer not acknowledged")
        }
        stage = "answer notification"
        try require(try await updates.next() != nil, "answer did not notify subscribers")
        stage = "answered dashboard read"
        let answered = try await client.dashboard()
        try require(answered.approvals.contains { $0.id == id && $0.entry.answer == "always allow" }, "answer not persisted")
        stage = "remove history"
        guard case .acknowledged = try await client.send(.removeHistory(id: id)) else {
            throw ServiceError(message: "removal not acknowledged")
        }
        stage = "removed dashboard read"
        let removed = try await client.dashboard()
        try require(removed.approvals.isEmpty, "removed history still listed")
        stage = "inspect missing offer"
        guard case .offer(nil) = try await client.send(.inspectOffer(id: "gone")) else {
            throw ServiceError(message: "missing connector offer was not explicit")
        }
        stage = "live connector changes from another client"
        try await verifyLiveConnectorChanges(client, source: CommandLine.arguments[3])
        stage = "initial approval subscription"
        var live = try client.replies(to: .watchApprovals).makeAsyncIterator()
        guard let bytes = try await live.next(), case let .snapshot(approvals) = try ServiceReply.decode(bytes) else {
            throw ServiceError(message: "live approval subscription failed")
        }
        try require(approvals.approvals.isEmpty, "unexpected live request")
        stage = "configuration preview, live decisions, and save"
        try await verifyConfiguration(client, root: URL(fileURLWithPath: CommandLine.arguments[3]).deletingLastPathComponent())
        stage = "packaged component sign-in"
        try await verifyComponentSignIn(client, source: URL(fileURLWithPath: CommandLine.arguments[3]).deletingLastPathComponent().appendingPathComponent("code-connector.yaml").path)
        stage = "shutdown acknowledgment"
        guard case .shuttingDown = try await client.send(.shutdown) else {
            throw ServiceError(message: "shutdown not acknowledged")
        }
        stage = "dashboard subscription shutdown"
        while try await updates.next() != nil {}
        stage = "approval subscription shutdown"
        try require(try await live.next() == nil, "shutdown did not close approval subscription")
        stage = "complete"
        print("PASS: Swift client and real service agree on audit, history, configuration, mixin preview, saved definitions, connectors, notifications, and shutdown")
    }

    static func verifyComponentSignIn(_ client: ServiceConnection, source: String) async throws {
        guard case .completed = try await client.manage(.install(source)) else {
            throw ServiceError(message: "component connector installation failed")
        }
        guard case let .connectAsk(ask) = try await client.manage(.beginConnect("component-smoke", method: "sign-in", label: "smoke")) else {
            throw ServiceError(message: "packaged service could not execute the component")
        }
        try require(ask.from_code && ask.fields.map(\.name) == ["workspace", "access_token"], "component fields did not reach the native client")
        try require(!ask.fields[0].secret && ask.fields[1].secret, "component field secrecy was lost")
        guard case .completed = try await client.manage(.answerConnect(ask.session, values: ["workspace": "test", "access_token": "non-secret-smoke-value"])) else {
            throw ServiceError(message: "component sign-in did not finish")
        }
        guard case let .connectors(connectors) = try await client.manage(.listConnectors) else {
            throw ServiceError(message: "component connection could not be read back")
        }
        try require(connectors.first { $0.name == "component-smoke" }?.connections.contains { $0.label == "smoke" } == true, "component connection was not saved")
        print("PASS: packaged service executes a component and completes native sign-in")
    }

    static func verifyLiveConnectorChanges(_ client: ServiceConnection, source: String) async throws {
        var changes = try client.replies(to: .watchDashboard).makeAsyncIterator()
        try require(try await changes.next() != nil, "management subscription did not start")
        let other = ServiceConnection(path: CommandLine.arguments[1])
        guard case .completed = try await other.manage(.install(source)) else {
            throw ServiceError(message: "connector installation did not complete")
        }
        guard let installed = try await changes.next(),
              case .changed = try JSONDecoder().decode(DashboardMessage.self, from: installed) else {
            throw ServiceError(message: "connector installation did not notify the other client")
        }
        guard case let .connectors(connectors) = try await client.manage(.listConnectors) else {
            throw ServiceError(message: "connector inventory missing")
        }
        try require(connectors.first { $0.name == "issues" }?.description == "Work with projects and issues.",
                    "authored connector description did not reach the Swift client")
        guard case .completed = try await other.manage(.uninstall("issues")) else {
            throw ServiceError(message: "connector removal did not complete")
        }
        guard let removed = try await changes.next(),
              case .changed = try JSONDecoder().decode(DashboardMessage.self, from: removed) else {
            throw ServiceError(message: "connector removal did not notify the other client")
        }
        guard case let .connectors(remaining) = try await client.manage(.listConnectors) else {
            throw ServiceError(message: "updated connector inventory missing")
        }
        try require(!remaining.contains { $0.name == "issues" }, "removed connector remained in the other client's inventory")
    }

    @MainActor
    static func verifyConfiguration(_ client: ServiceConnection, root: URL) async throws {
        var draft = SandboxDraft()
        draft.source = root.appendingPathComponent("lns.yaml").path
        draft.mixins = [root.appendingPathComponent("tools.yaml").path]
        guard case let .configuration(preview) = try await client.send(.management(.preview(draft))) else {
            throw ServiceError(message: "configuration preview missing")
        }
        try require(preview.spec["tools"] as? [String] == ["node@22"], "preview did not merge the added mixin")
        try require(preview.sources?.added_mixins == draft.mixins, "preview lost mixin provenance")
        guard case let .configuration(current) = try await client.send(.management(.configuration("quiet_river"))) else {
            throw ServiceError(message: "current configuration missing")
        }
        try require(current.userDecisions.contains { $0.destination == "example.com" && $0.verdict == "allow" }, "live decision missing after history was removed")
        let saving = SandboxSaving(service: client) { try $1.write(to: $0, options: .withoutOverwriting) }
        let saved = await saving.save(run: "quiet_river", to: root.appendingPathComponent("saved-reviewer.yaml"))
        try require(saved, saving.error ?? "save failed")
        let overwritten = await saving.save(run: "quiet_river", to: root.appendingPathComponent("saved-reviewer.yaml"))
        try require(!overwritten, "save overwrote an existing file")
    }
}
