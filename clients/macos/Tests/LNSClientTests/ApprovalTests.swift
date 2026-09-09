import Foundation
import XCTest
@testable import LNSClient

final class ApprovalTests: XCTestCase {
    func testDisconnectDisablesAnswersAndReconnectReplacesOldState() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "live-approvals", withExtension: "json", subdirectory: "Fixtures"))
        let snapshot = try JSONDecoder().decode(ApprovalSnapshot.self, from: Data(contentsOf: url))
        var feed = ApprovalFeed()
        feed.receive(snapshot)
        XCTAssertTrue(feed.connected)
        XCTAssertEqual(feed.snapshot.approvals.count, 1)
        feed.disconnect()
        XCTAssertFalse(feed.connected, "a disconnected client must disable answering")
        XCTAssertTrue(feed.snapshot.approvals.isEmpty, "old approvals must not stay actionable")
        feed.receive(.empty)
        XCTAssertTrue(feed.connected)
        XCTAssertEqual(feed.snapshot, .empty)
    }
    func testDecodesTheFixtureProducedByRust() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "live-approvals", withExtension: "json", subdirectory: "Fixtures"))
        guard case let .snapshot(snapshot) = try ServiceReply.decode(Data(contentsOf: url)) else {
            return XCTFail("expected a live approval snapshot")
        }
        XCTAssertEqual(snapshot.approvals.count, 1)
        XCTAssertEqual(snapshot.approvals[0].token, "presentation-1")
        XCTAssertEqual(snapshot.approvals[0].run, "quiet_river")
        XCTAssertFalse(snapshot.approvals[0].raw)
        XCTAssertTrue(snapshot.approvals[0].waiting)
        XCTAssertEqual(snapshot.notices, ["A decision could not be saved."])
    }

    func testAResponseTargetsTheObservedPresentation() throws {
        let data = try JSONEncoder().encode(ServiceRequest.respond(token: "presentation-1", action: .allowOnce))
        let actual = try JSONSerialization.jsonObject(with: data) as? NSDictionary
        let expected: NSDictionary = ["type": "RespondToApproval", "token": "presentation-1", "action": ["kind": "allow_once"]]
        XCTAssertEqual(actual, expected)
    }

    func testAConnectorGrantPreservesFieldNamesAndConnectionChoice() throws {
        let action = ApprovalAction.grant(method: "token", connection: .new(label: "work", values: ["API_TOKEN": "test-value"]))
        let data = try JSONEncoder().encode(ServiceRequest.respond(token: "presentation-2", action: action))
        let actual = try JSONSerialization.jsonObject(with: data) as? NSDictionary
        let expected: NSDictionary = ["type": "RespondToApproval", "token": "presentation-2", "action": [
            "kind": "grant", "method": "token", "connection": ["kind": "new", "label": "work", "values": ["API_TOKEN": "test-value"]]
        ]]
        XCTAssertEqual(actual, expected)
    }

    func testServiceErrorsAndUnknownRepliesAreNotSuccessfulAnswers() {
        for json in [#"{"type":"Error","message":"could not save"}"#, #"{"type":"FutureReply"}"#] {
            XCTAssertThrowsError(try ServiceReply.decode(Data(json.utf8)))
        }
    }
}
