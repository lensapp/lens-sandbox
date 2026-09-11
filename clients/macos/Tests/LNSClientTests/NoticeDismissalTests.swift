import Foundation
import XCTest
@testable import LNSClient

final class NoticeDismissalTests: XCTestCase {
    func testLargeNoticeDismissalsStayBoundedAndPreserveExactlyTheObservedNotices() throws {
        let notices = (0..<1024).map { "\($0): " + String(repeating: "é🦀\n\"\\", count: 256) }
        let requests = try ServiceRequest.noticeDismissalBatches(notices)
        XCTAssertEqual(requests.flatMap { $0.notices ?? [] }, notices)
        for request in requests {
            XCTAssertNoThrow(try FrameDecoder.encode(JSONEncoder().encode(request)), "clearing a large snapshot must not send an oversized command")
        }
    }

    func testAnIndividuallyOversizedNoticeIsRefusedBeforeSendingAnyBatch() {
        XCTAssertThrowsError(try ServiceRequest.noticeDismissalBatches(["normal", String(repeating: "x", count: 1_048_576)]))
    }

    func testNoNoticesNeedNoCommands() throws {
        XCTAssertTrue(try ServiceRequest.noticeDismissalBatches([]).isEmpty)
    }
}
