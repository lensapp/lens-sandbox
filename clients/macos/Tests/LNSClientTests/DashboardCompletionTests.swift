import Foundation
import XCTest
@testable import LNSClient

final class DashboardCompletionTests: XCTestCase {
    func replies(complete: Bool, error: Error?) -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            continuation.yield(Data(#"{"type":"DashboardBegin"}"#.utf8))
            continuation.yield(Data(#"{"type":"DashboardWarning","message":"retained warning"}"#.utf8))
            if complete { continuation.yield(Data(#"{"type":"DashboardEnd"}"#.utf8)) }
            continuation.finish(throwing: error)
        }
    }

    func testAnExplicitEndCompletesTheReadBeforeATransportCloseError() async throws {
        let snapshot = try await readDashboard(replies(complete: true, error: ServiceError(message: "Network is down")))
        XCTAssertEqual(snapshot.warnings, ["retained warning"])
    }

    func testATransportErrorBeforeEndMustStillFailTheRead() async {
        do {
            _ = try await readDashboard(replies(complete: false, error: ServiceError(message: "Network is down")))
            XCTFail("a read without DashboardEnd must not publish partial data")
        } catch {
            XCTAssertEqual(error.localizedDescription, "Network is down")
        }
    }

    func testEvenACleanSocketCloseCannotReplaceTheExplicitEnd() async {
        do {
            _ = try await readDashboard(replies(complete: false, error: nil))
            XCTFail("EOF without DashboardEnd must not publish partial data")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("before its snapshot was complete"))
        }
    }
}
