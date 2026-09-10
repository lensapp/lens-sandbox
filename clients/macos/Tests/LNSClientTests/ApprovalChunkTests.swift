import Foundation
import XCTest
@testable import LNSClient

final class ApprovalChunkTests: XCTestCase {
    func chunk(_ text: String, offset: Int, complete: Bool) throws -> Data {
        try FrameDecoder.encode(JSONSerialization.data(withJSONObject: ["type": "LiveApprovalsChunk", "offset": offset, "data": text, "complete": complete]))
    }

    func testOnlyCompleteSnapshotsReachTheLatestOnlySubscriptionBuffer() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let prefix = "{\"type\":\"LiveApprovals\",\"approvals\":[],\"notices\":[\"é🦀"
        let suffix = " could not save\"]}"
        _ = try reader.receive(chunk(prefix, offset: 0, complete: false), complete: false, error: nil, once: false) { received.append($0) }
        XCTAssertTrue(received.isEmpty, "partial snapshot chunks must not displace a complete snapshot")
        _ = try reader.receive(chunk(suffix, offset: prefix.utf8.count, complete: true), complete: false, error: nil, once: false) { received.append($0) }
        XCTAssertEqual(received, [Data((prefix + suffix).utf8)])
    }

    func testInterruptedOrOutOfOrderChunksCannotPublishASnapshot() throws {
        for offset in [0, 1, 3] {
            var reader = ReplyRead()
            _ = try reader.receive(chunk("ab", offset: 0, complete: false), complete: false, error: nil, once: false) { _ in }
            XCTAssertThrowsError(try reader.receive(chunk("c", offset: offset, complete: true), complete: false, error: nil, once: false) { _ in XCTFail("invalid snapshot published") })
        }
        var reader = ReplyRead()
        _ = try reader.receive(chunk("ab", offset: 0, complete: false), complete: false, error: nil, once: false) { _ in }
        XCTAssertThrowsError(try reader.receive(nil, complete: true, error: nil, once: false) { _ in })
    }
}
