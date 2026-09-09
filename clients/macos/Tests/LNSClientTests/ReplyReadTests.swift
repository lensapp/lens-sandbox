import Foundation
import XCTest
@testable import LNSClient

final class ReplyReadTests: XCTestCase {
    func testAStateFailureCannotCancelAnAlreadyScheduledSendOrReceive() {
        var lifecycle = ReplyLifecycle()
        XCTAssertTrue(lifecycle.acceptsConnectionFailure)
        XCTAssertTrue(lifecycle.start())
        XCTAssertFalse(lifecycle.acceptsConnectionFailure, "the pending I/O callback must deliver its final bytes before finishing")
        XCTAssertFalse(lifecycle.start())
        XCTAssertTrue(lifecycle.finish())
        XCTAssertFalse(lifecycle.finish())
        XCTAssertFalse(lifecycle.acceptsConnectionFailure)
        XCTAssertFalse(lifecycle.start())
    }

    func testCancellationBeforeReadinessCannotStartARequestLater() {
        var lifecycle = ReplyLifecycle()
        XCTAssertTrue(lifecycle.finish())
        XCTAssertFalse(lifecycle.start())
    }

    func testAnAcknowledgmentDeliveredWithACloseErrorIsNotLost() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let payload = Data(#"{"type":"Acknowledged"}"#.utf8)
        let ended = try reader.receive(FrameDecoder.encode(payload), complete: true, error: ServiceError(message: "closed"), once: true) { received.append($0) }
        XCTAssertTrue(ended)
        XCTAssertEqual(received, [payload])
    }

    func testStreamingFramesAreDeliveredBeforeTheirTerminalError() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let payload = Data(#"{"type":"DashboardEnd"}"#.utf8)
        XCTAssertThrowsError(try reader.receive(FrameDecoder.encode(payload), complete: true, error: ServiceError(message: "closed"), once: false) { received.append($0) }) { error in
            XCTAssertEqual(error.localizedDescription, "closed")
        }
        XCTAssertEqual(received, [payload], "a close error must not discard already received frames")
    }

    func testPartialFramesKeepReadingAndTruncatedEOFStillFails() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let frame = try FrameDecoder.encode(Data("reply".utf8))
        XCTAssertFalse(try reader.receive(frame.prefix(5), complete: false, error: nil, once: false) { received.append($0) })
        XCTAssertTrue(received.isEmpty)
        XCTAssertThrowsError(try reader.receive(nil, complete: true, error: nil, once: false) { received.append($0) })
        XCTAssertTrue(received.isEmpty)
    }

    func testACloseErrorWithoutACompleteReplyNeverAcknowledgesTheRequest() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let frame = try FrameDecoder.encode(Data("reply".utf8))
        XCTAssertThrowsError(try reader.receive(frame.prefix(5), complete: true, error: ServiceError(message: "closed"), once: true) { received.append($0) }) { error in
            XCTAssertEqual(error.localizedDescription, "closed")
        }
        XCTAssertTrue(received.isEmpty)
    }

    func testACompleteStreamingFrameCanBeFollowedByCleanEOF() throws {
        var reader = ReplyRead()
        var received: [Data] = []
        let payload = Data("reply".utf8)
        XCTAssertFalse(try reader.receive(FrameDecoder.encode(payload), complete: false, error: nil, once: false) { received.append($0) })
        XCTAssertEqual(received, [payload])
        XCTAssertTrue(try reader.receive(nil, complete: true, error: nil, once: false) { received.append($0) })
    }
}
