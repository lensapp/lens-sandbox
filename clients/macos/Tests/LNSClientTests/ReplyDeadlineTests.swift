import XCTest
@testable import LNSClient

final class ReplyDeadlineTests: XCTestCase {
    func testAConnectionWithoutACompleteReplyTimesOut() {
        let deadline = ReplyDeadline(streaming: true, now: 100)
        XCTAssertFalse(deadline.expired(now: 109))
        XCTAssertTrue(deadline.expired(now: 110), "a silent service must not leave a client waiting forever")
    }

    func testAFiniteDashboardReadMustKeepMakingProgressUntilEOF() {
        var deadline = ReplyDeadline(streaming: false, now: 100)
        deadline.received(now: 109)
        XCTAssertFalse(deadline.expired(now: 110))
        XCTAssertTrue(deadline.expired(now: 119), "a partial dashboard must time out too")
        deadline.received(now: 118)
        XCTAssertFalse(deadline.expired(now: 119))
        XCTAssertTrue(deadline.expired(now: 128))
    }

    func testAnEstablishedSubscriptionMayStayIdleIndefinitely() {
        var deadline = ReplyDeadline(streaming: true, now: 100)
        deadline.received(now: 101)
        XCTAssertFalse(deadline.expired(now: 1_000_000), "idle subscriptions must not reconnect on a timer")
    }
}
