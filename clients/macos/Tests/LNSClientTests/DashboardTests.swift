import Foundation
import XCTest
@testable import LNSClient

final class DashboardTests: XCTestCase {
    func testHistoryRepliesDistinguishAcceptedActionsFromPersistenceFailures() throws {
        for type in ["Acknowledged", "ApprovalAnswered", "ApprovalRemoved"] {
            guard case .acknowledged = try ServiceReply.decode(Data("{\"type\":\"\(type)\"}".utf8)) else {
                return XCTFail("expected an acknowledged history action")
            }
        }
        guard case .offer(nil) = try ServiceReply.decode(Data(#"{"type":"ApprovalOffer","offer":null}"#.utf8)) else {
            return XCTFail("an unavailable offer is explicit")
        }
        for type in ["ApprovalNotWritten", "ApprovalKept"] {
            XCTAssertThrowsError(try ServiceReply.decode(Data("{\"type\":\"\(type)\",\"id\":\"entry-1\",\"reason\":\"disk full\"}".utf8))) { error in
                XCTAssertEqual(error.localizedDescription, "disk full")
            }
        }
        XCTAssertThrowsError(try ServiceReply.decode(Data(#"{"type":"ApprovalUnknown","id":"gone"}"#.utf8)))
    }
    func testHistoryCommandsNameTheEntryAndNeverMasqueradeAsLiveAnswers() throws {
        let requests: [(ServiceRequest, NSDictionary)] = [
            (.dismissNotices(["old warning"]), ["type": "DismissApprovalNotices", "notices": ["old warning"]]),
            (.answerHistory(id: "entry-1", answer: .askAgain), ["type": "AnswerApproval", "id": "entry-1", "answer": "ask-again"]),
            (.removeHistory(id: "entry-1"), ["type": "RemoveApproval", "id": "entry-1"]),
            (.inspectOffer(id: "entry-1"), ["type": "InspectApprovalOffer", "id": "entry-1"]),
            (.grantHistory(id: "entry-1", method: "token", digest: "sha256:test", connection: .held(label: "work")), ["type": "GrantApproval", "id": "entry-1", "method": "token", "digest": "sha256:test", "connection": ["kind": "held", "label": "work"]]),
        ]
        for (request, expected) in requests {
            XCTAssertEqual(try JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? NSDictionary, expected)
        }
    }
    func fixture() throws -> [DashboardMessage] {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "dashboard", withExtension: "json", subdirectory: "Fixtures"))
        return try JSONDecoder().decode([DashboardMessage].self, from: Data(contentsOf: url))
    }

    func data() throws -> DashboardData {
        var reader = DashboardRead()
        var result: DashboardData?
        for message in try fixture() { if let snapshot = try reader.receive(message) { result = snapshot } }
        try reader.finish()
        return try XCTUnwrap(result, "a complete dashboard must publish its data")
    }

    func testDashboardPublishesOnlyAfterCompletionAndPreservesIntegrityWarningsAndRawNumbers() throws {
        var reader = DashboardRead()
        let messages = try fixture()
        for message in messages.dropLast() { XCTAssertNil(try reader.receive(message)) }
        let snapshot = try XCTUnwrap(reader.receive(try XCTUnwrap(messages.last)))
        try reader.finish()
        XCTAssertEqual(snapshot.sandboxes[0].name, "quiet_river")
        XCTAssertEqual(snapshot.events[0].detail, "CONNECT example.com:443")
        XCTAssertTrue(snapshot.events[0].raw.contains("9007199254740993"))
        XCTAssertTrue(snapshot.approvals[0].raw)
        XCTAssertEqual(snapshot.approvals[0].answers, [.alwaysAllow, .alwaysDeny])
        XCTAssertTrue(snapshot.warnings[0].contains("truncated"))
    }

    func testInterruptedAndMalformedSnapshotsAreNotTreatedAsEmptySuccess() throws {
        var empty = DashboardRead()
        XCTAssertThrowsError(try empty.finish())
        XCTAssertThrowsError(try empty.receive(.end))
        var partial = DashboardRead()
        for message in try fixture().dropLast() { _ = try partial.receive(message) }
        XCTAssertThrowsError(try partial.finish())
        XCTAssertThrowsError(try partial.receive(.begin))
        XCTAssertThrowsError(try partial.receive(.changed))
        _ = try partial.receive(.end)
        XCTAssertThrowsError(try partial.receive(.end))
        XCTAssertThrowsError(try partial.receive(.warning("late")))
    }

    func testEventFiltersComposeButSearchIsAcrossAllSandboxes() throws {
        let snapshot = try data()
        var filters = DashboardFilters()
        filters.sandbox = "elsewhere"
        XCTAssertTrue(filters.events(in: snapshot).isEmpty)
        filters.sandbox = "run-1"
        filters.kinds = ["launch"]
        XCTAssertTrue(filters.events(in: snapshot).isEmpty)
        filters.kinds = ["egress", "launch"]
        XCTAssertEqual(filters.events(in: snapshot).count, 1)
        filters.sandbox = "elsewhere"
        filters.search = "  EXAMPLE.COM  "
        XCTAssertEqual(filters.events(in: snapshot).count, 1, "search covers every sandbox, like the current audit search")
        filters.search = "absent"
        XCTAssertTrue(filters.events(in: snapshot).isEmpty)
    }

    func testApprovalFiltersResolveRunNamesAndDoNotHideTheWaitingBadge() throws {
        let snapshot = try data()
        var filters = DashboardFilters()
        filters.sandbox = "run-1"
        XCTAssertEqual(filters.approvals(in: snapshot).count, 1)
        filters.answers = ["always allow"]
        XCTAssertTrue(filters.approvals(in: snapshot).isEmpty)
        XCTAssertEqual(filters.waitingCount(in: snapshot), 1, "answer filtering must not conceal unanswered requests")
        filters.sandbox = "elsewhere"
        XCTAssertEqual(filters.waitingCount(in: snapshot), 0)
    }

    func testDisconnectClearsActionableHistoryAndReconnectReplacesTheSnapshot() throws {
        var feed = DashboardFeed()
        feed.receive(try data())
        XCTAssertTrue(feed.connected)
        feed.disconnect()
        XCTAssertFalse(feed.connected)
        XCTAssertTrue(feed.data.approvals.isEmpty)
        feed.receive(DashboardData())
        XCTAssertTrue(feed.connected)
        XCTAssertTrue(feed.data.events.isEmpty)
    }
}
