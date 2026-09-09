import Foundation
import XCTest
@testable import LNSClient

@MainActor
final class ServiceControlTests: XCTestCase {
    final class Client: ServiceClient {
        var requests: [String] = []
        var reply = ServiceReply.shuttingDown
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
            throw ServiceError(message: "unexpected subscription")
        }
        func send(_ request: ServiceRequest) async throws -> ServiceReply { requests.append(request.type); return reply }
        func dashboard() async throws -> DashboardData { throw ServiceError(message: "unexpected dashboard") }
    }

    func testStartingIsExplicitAndDuplicateClicksCannotLaunchTwoHelpers() async {
        let client = Client()
        let started = expectation(description: "helper started")
        var finish: CheckedContinuation<Void, Error>?
        var launches = 0
        let control = ServiceControl(client: client, launch: {
            launches += 1
            try await withCheckedThrowingContinuation { finish = $0; started.fulfill() }
        }, confirm: { false }, quit: { XCTFail("start cannot quit") })
        XCTAssertEqual(launches, 0)
        let task = Task { await control.start() }
        await fulfillment(of: [started], timeout: 1)
        XCTAssertEqual(control.phase, .starting)
        await control.start()
        XCTAssertEqual(launches, 1, "duplicate start must not create another helper")
        finish?.resume()
        await task.value
        XCTAssertEqual(control.phase, .idle)
        XCTAssertTrue(client.requests.isEmpty)
    }

    func testStopRequiresConfirmationAndAServiceAcknowledgmentBeforeQuitting() async {
        let client = Client()
        var confirmed = false
        var confirmations = 0
        var quits = 0
        var errors: [String] = []
        let control = ServiceControl(client: client, launch: nil, confirm: {
            confirmations += 1; return confirmed
        }, quit: { quits += 1 })
        control.onError = { errors.append($0) }
        XCTAssertFalse(control.canStart)
        await control.stop(connected: false)
        XCTAssertEqual(confirmations, 0)
        await control.stop(connected: true)
        XCTAssertTrue(client.requests.isEmpty)
        confirmed = true
        client.reply = .acknowledged
        await control.stop(connected: true)
        XCTAssertEqual(client.requests, ["Shutdown"])
        XCTAssertEqual(quits, 0)
        XCTAssertEqual(errors.count, 1, "an unexpected reply must leave the interface open with an error")
        client.reply = .shuttingDown
        await control.stop(connected: true)
        XCTAssertEqual(quits, 1)
        XCTAssertEqual(control.phase, .idle)
    }

    func testFailedStartsSurfaceTheErrorAndMayBeRetriedExplicitly() async {
        let client = Client()
        var attempts = 0
        var errors: [String] = []
        let control = ServiceControl(client: client, launch: {
            attempts += 1
            throw ServiceError(message: "helper signature invalid")
        }, confirm: { false }, quit: {})
        control.onError = { errors.append($0) }
        await control.start()
        XCTAssertEqual(attempts, 1)
        XCTAssertEqual(errors, ["helper signature invalid"])
        XCTAssertTrue(control.canStart)
        await control.start()
        XCTAssertEqual(attempts, 2)
    }
}
