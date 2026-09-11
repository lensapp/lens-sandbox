import XCTest
@testable import LNSClient

@MainActor
final class SandboxConfigurationTests: XCTestCase {
    func testDecisionTimestampComesFromAMatchingPersistentApprovalForThisRun() {
        let rule = ConfigurationRule(table: "http", source: "Your decision", rule: #"{"match":"example.com","verdict":"deny"}"#)
        let event = DashboardEvent(id: "one", ts: "2026-09-11T12:00:00Z", when: "12:00", run: "run-1", kind: "approval", detail: "deny", raw: #"{"unmapped":{"lns_target":"example.com","lns_decision":"deny_always","lns_approval_kind":"network"}}"#)
        XCTAssertEqual(rule.latestApproval(in: [event], run: "run-1"), "12:00")
        XCTAssertNil(rule.latestApproval(in: [event], run: "run-2"))
        let once = DashboardEvent(id: "two", ts: "2026-09-11T13:00:00Z", when: "13:00", run: "run-1", kind: "approval", detail: "deny", raw: event.raw.replacingOccurrences(of: "deny_always", with: "deny_once"))
        XCTAssertNil(rule.latestApproval(in: [once], run: "run-1"))
    }
    func testNativeConfigurationCommandsAndRepliesUseTheSharedRustContract() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "configuration", withExtension: "json", subdirectory: "Fixtures"))
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        var draft = SandboxDraft(); draft.source = "/project/lns.yaml"; draft.mixins = ["/tools/lns.yaml"]
        let requests = [ManagementCommand.configuration("run-1"), .preview(draft), .save("run-1", name: "reviewer")].map(ServiceRequest.management)
        XCTAssertEqual(try JSONSerialization.jsonObject(with: JSONEncoder().encode(requests)) as? NSArray, fixture["requests"] as? NSArray)
        let responses = try XCTUnwrap(fixture["responses"] as? [[String: Any]])
        guard case let .configuration(config) = try ServiceReply.decode(JSONSerialization.data(withJSONObject: responses[0])) else { return XCTFail("configuration response missing") }
        XCTAssertEqual(config.userDecisions.first?.destination, "example.com")
        XCTAssertEqual(config.sources?.added_mixins, draft.mixins)
        XCTAssertEqual(config.spec["tools"] as? [String], ["node@22"])
        guard case .savedDocument("kind: sandbox\n") = try ServiceReply.decode(JSONSerialization.data(withJSONObject: responses[1])) else { return XCTFail("saved document missing") }
    }

    func testMalformedConfigurationIsAnErrorRatherThanAnEmptyConfiguration() throws {
        let text = #"{"type":"SandboxConfiguration","configuration":{"sources":null,"document":"not json","decisions":"{}","grants":[],"rules":[]}}"#
        XCTAssertThrowsError(try ServiceReply.decode(Data(text.utf8)))
    }
    func testInspectionExplainsAUserDecisionOverridingADocumentRule() {
        let rules = [
            ConfigurationRule(table: "http", source: "Your decision", rule: #"{"match":"example.com","verdict":"deny"}"#),
            ConfigurationRule(table: "http", source: "team-network", rule: #"{"match":"example.com","verdict":"allow"}"#),
            ConfigurationRule(table: "tcp", source: "team-network", rule: #"{"match":"example.com:443","verdict":"allow"}"#),
        ]
        let config = SandboxConfiguration(sources: nil, document: "{}", decisions: "{}", grants: [], rules: rules)
        XCTAssertEqual(config.overridingSource(for: 1), "Your decision")
        XCTAssertNil(config.overridingSource(for: 0))
        XCTAssertNil(config.overridingSource(for: 2))
    }

    final class Client: ServiceClient {
        var reply = ServiceReply.acknowledged
        var requests: [ServiceRequest] = []
        var pending: CheckedContinuation<ServiceReply, Error>?
        var hold = false
        func send(_ request: ServiceRequest) async throws -> ServiceReply {
            requests.append(request)
            if hold { return try await withCheckedThrowingContinuation { pending = $0 } }
            return reply
        }
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> { fatalError("unexpected stream") }
        func dashboard() async throws -> DashboardData { DashboardData() }
    }

    func testSavingWritesOnlyTheServiceDocumentAndUsesTheChosenFilename() async throws {
        let client = Client(); client.reply = .savedDocument("kind: sandbox\n")
        var writes: [(URL, Data)] = []
        let saving = SandboxSaving(service: client) { writes.append(($0, $1)) }
        let file = URL(fileURLWithPath: "/saved/reviewer.yaml")
        let saved = await saving.save(run: "run-1", to: file)
        XCTAssertTrue(saved)
        XCTAssertEqual(writes.first?.0, file)
        XCTAssertEqual(writes.first?.1, Data("kind: sandbox\n".utf8))
        let request = try XCTUnwrap(client.requests.first)
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any])
        XCTAssertEqual(fields["name"] as? String, "reviewer")
        XCTAssertEqual(fields["kind"] as? String, "sandbox")
    }

    func testSavingRefusesInvalidNamesAndReportsWriteFailures() async {
        let client = Client(); client.reply = .savedDocument("kind: sandbox\n")
        let saving = SandboxSaving(service: client) { _, _ in throw ServiceError(message: "File already exists") }
        let invalid = await saving.save(run: "run-1", to: URL(fileURLWithPath: "/saved/Not Valid.yaml"))
        XCTAssertFalse(invalid); XCTAssertTrue(client.requests.isEmpty)
        let refused = await saving.save(run: "run-1", to: URL(fileURLWithPath: "/saved/reviewer.yaml"))
        XCTAssertFalse(refused); XCTAssertEqual(saving.error, "File already exists")
    }

    func testLateConfigurationCannotRestoreDataAfterClosingOrDisconnecting() async {
        let client = Client(); client.hold = true
        let session = ConfigurationSession(service: client)
        let read = Task { await session.read(.configuration("run-1")) }
        while client.pending == nil { await Task.yield() }
        session.clear()
        client.pending?.resume(returning: .configuration(SandboxConfiguration(sources: nil, document: "{}", decisions: "{}", grants: [], rules: [])))
        await read.value
        XCTAssertNil(session.configuration); XCTAssertFalse(session.loading)
    }

    func testRefreshingTheSameSandboxKeepsItsContentUntilTheReadCompletes() async {
        let client = Client()
        client.reply = .configuration(SandboxConfiguration(sources: nil, document: "{}", decisions: "{}", grants: [], rules: []))
        let session = ConfigurationSession(service: client)
        await session.read(.configuration("run-1"))
        client.hold = true
        let refresh = Task { await session.read(.configuration("run-1")) }
        while client.pending == nil { await Task.yield() }
        XCTAssertNotNil(session.configuration, "live refresh must not collapse the inspector or lose its disclosure state")
        client.pending?.resume(throwing: ServiceError(message: "disconnected"))
        await refresh.value
        XCTAssertNil(session.configuration)
        XCTAssertEqual(session.error, "disconnected")
    }
}
