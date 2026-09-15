import Foundation
import XCTest
@testable import LNSClient

@MainActor
final class ManagementSessionTests: XCTestCase {
    final class Client: ServiceClient {
        var requests: [ServiceRequest] = []
        var frames: [String] = []
        var streams: [AsyncThrowingStream<Data, Error>] = []
        var onRequest: ((ServiceRequest) -> Void)?
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
            requests.append(request)
            onRequest?(request)
            XCTAssertFalse(once, "management must read past progress frames")
            XCTAssertFalse(latestOnly, "management cannot drop the terminal acknowledgment")
            if !streams.isEmpty { return streams.removeFirst() }
            return AsyncThrowingStream { continuation in
                frames.forEach { continuation.yield(Data($0.utf8)) }
                continuation.finish()
            }
        }
        func send(_ request: ServiceRequest) async throws -> ServiceReply { XCTFail("unexpected one-shot request"); return .acknowledged }
        func dashboard() async throws -> DashboardData { DashboardData() }
    }

    func testStartConsumesProgressAndStopsAtItsAcknowledgmentWithoutWaitingForEOF() async throws {
        let client = Client()
        client.streams = [AsyncThrowingStream { continuation in
            continuation.yield(Data(#"{"type":"RunProgress","verb":"Booting","message":"guest","current":1,"total":2}"#.utf8))
            continuation.yield(Data(#"{"type":"RunLog","level":"info","verb":"Starting","message":"guest"}"#.utf8))
            continuation.yield(Data(#"{"type":"RunStarted","run_id":"run-1"}"#.utf8))
        }]
        guard case .completed("Sandbox started.") = try await client.manage(.start("run-1")) else { return XCTFail("start must finish at RunStarted") }
        XCTAssertEqual(client.requests.map(\.type), ["StartRun"])
    }

    func testUnexpectedAcknowledgmentOrProgressOnlyDisconnectCannotClaimSuccess() async {
        let client = Client()
        for frames in [[#"{"type":"Acknowledged"}"#], [#"{"type":"RunLog","level":"info","message":"starting"}"#], [#"{"type":"Error","message":"boot failed"}"#]] {
            client.frames = frames
            do { _ = try await client.manage(.start("run-1")); XCTFail("start was not confirmed") }
            catch { XCTAssertFalse(error.localizedDescription.isEmpty) }
        }
    }

    func testInventoryDisconnectClearsDataAndDiscardsALateRead() async {
        let client = Client()
        client.frames = [#"{"type":"ConnectorList","connectors":[{"name":"github","digest":"one","serves":[],"methods":[],"connections":[]}]}"#]
        let session = ManagementSession(service: client)
        await session.refresh()
        XCTAssertTrue(session.connected)
        XCTAssertEqual(session.connectors.count, 1)
        var pending: AsyncThrowingStream<Data, Error>.Continuation?
        client.streams = [AsyncThrowingStream { pending = $0 }]
        let started = expectation(description: "inventory read started")
        client.onRequest = { _ in started.fulfill() }
        let reading = Task { await session.refresh() }
        await fulfillment(of: [started], timeout: 1)
        session.disconnect()
        pending?.yield(Data(client.frames[0].utf8)); pending?.finish()
        await reading.value
        XCTAssertFalse(session.connected)
        XCTAssertTrue(session.connectors.isEmpty)
    }

    func testDuplicateActionsAreSuppressedAndLostRepliesAreNeverRetried() async {
        let client = Client()
        client.frames = [#"{"type":"ConnectorList","connectors":[]}"#]
        let session = ManagementSession(service: client)
        await session.refresh()
        var pending: AsyncThrowingStream<Data, Error>.Continuation?
        client.streams = [AsyncThrowingStream { pending = $0 }]
        let sent = expectation(description: "stop sent")
        client.onRequest = { if $0.type == "StopRun" { sent.fulfill() } }
        let stopping = Task { await session.perform(.stop("run-1")) }
        await fulfillment(of: [sent], timeout: 1)
        XCTAssertTrue(session.busy)
        let duplicate = await session.perform(.stop("run-1"))
        XCTAssertFalse(duplicate)
        pending?.finish()
        let result = await stopping.value
        XCTAssertFalse(result)
        XCTAssertFalse(session.busy)
        XCTAssertNil(session.message)
        XCTAssertTrue(session.error?.contains("before confirming") == true)
        XCTAssertEqual(client.requests.map(\.type), ["ListConnectors", "StopRun"])
    }

    func testConfirmedActionRefreshesInventoryAndPreservesItsOutcome() async {
        let client = Client()
        client.frames = [#"{"type":"ConnectorList","connectors":[]}"#]
        let session = ManagementSession(service: client)
        await session.refresh()
        client.streams = [AsyncThrowingStream { continuation in
            continuation.yield(Data(#"{"type":"RunStopped","forced":false}"#.utf8)); continuation.finish()
        }]
        let result = await session.perform(.stop("run-1"))
        XCTAssertTrue(result)
        XCTAssertEqual(session.message, "Sandbox stopped.")
        XCTAssertEqual(client.requests.map(\.type), ["ListConnectors", "StopRun", "ListConnectors"])
        XCTAssertTrue(session.connected)
        XCTAssertFalse(session.busy)
    }

    func testDismissingAnActionMessageDoesNotRepeatTheActionAndAllowsTheNextResult() async {
        let client = Client()
        client.frames = [#"{"type":"ConnectorList","connectors":[]}"#]
        let session = ManagementSession(service: client)
        await session.refresh()
        for _ in 0..<2 {
            client.streams = [AsyncThrowingStream { continuation in
                continuation.yield(Data(#"{"type":"RunStopped","forced":false}"#.utf8)); continuation.finish()
            }]
            let result = await session.perform(.stop("run-1"))
            XCTAssertTrue(result)
            XCTAssertEqual(session.message, "Sandbox stopped.")
            let requests = client.requests.count
            var changed = false
            session.onChange = { changed = true }
            session.dismissMessage()
            XCTAssertNil(session.message)
            XCTAssertTrue(changed, "The banner must update when dismissed")
            XCTAssertEqual(client.requests.count, requests, "Dismissal must not submit an action")
            XCTAssertTrue(session.connected)
        }
    }

    func testBrokenConnectorInventoryCannotPreventStoppingASandbox() async {
        let client = Client()
        let session = ManagementSession(service: client)
        session.serviceConnected()
        client.frames = [#"{"type":"Error","message":"connector document is unreadable"}"#]
        await session.refresh()
        XCTAssertTrue(session.connected, "the dashboard already confirmed the service connection")
        client.streams = [AsyncThrowingStream { continuation in
            continuation.yield(Data(#"{"type":"RunStopped","forced":false}"#.utf8)); continuation.finish()
        }]
        let result = await session.perform(.stop("run-1"))
        XCTAssertTrue(result)
        XCTAssertEqual(session.message, "Sandbox stopped.")
        XCTAssertEqual(session.error, "connector document is unreadable")
        XCTAssertTrue(session.connectors.isEmpty)
    }

    func testCoalescedRefreshReadsAgainBeforePublishingAnActionOutcome() async {
        let client = Client()
        client.frames = [#"{"type":"ConnectorList","connectors":[]}"#]
        let session = ManagementSession(service: client)
        var pending: AsyncThrowingStream<Data, Error>.Continuation?
        client.streams = [AsyncThrowingStream { pending = $0 }]
        let started = expectation(description: "inventory read started")
        client.onRequest = { _ in started.fulfill() }
        let first = Task { await session.refresh() }
        await fulfillment(of: [started], timeout: 1)
        client.onRequest = nil
        let joined = expectation(description: "second refresh joined")
        let second = Task {
            joined.fulfill()
            await session.refresh()
        }
        await fulfillment(of: [joined], timeout: 1)
        pending?.yield(Data(#"{"type":"ConnectorList","connectors":[{"name":"old","digest":"one","serves":[],"methods":[],"connections":[]}]}"#.utf8))
        pending?.finish()
        await first.value; await second.value
        XCTAssertEqual(client.requests.count, 2)
        XCTAssertTrue(session.connectors.isEmpty)
        XCTAssertFalse(session.loading)
    }

    func testChangedGrantDisclosureAndDisconnectedActionsSendNoMutation() async throws {
        let client = Client()
        let session = ManagementSession(service: client)
        let command = ManagementCommand.grant("github", run: "run-1", method: "public", connection: nil)
        let disconnected = await session.perform(command)
        XCTAssertFalse(disconnected)
        XCTAssertTrue(client.requests.isEmpty)
        client.frames = [#"{"type":"ConnectorList","connectors":[{"name":"github","digest":"one","serves":[],"methods":[],"connections":[]}]}"#]
        await session.refresh()
        let offer = try XCTUnwrap(session.connectors.first)
        client.frames = [#"{"type":"ConnectorList","connectors":[]}"#]
        let result = await session.perform(command, reviewing: offer)
        XCTAssertFalse(result)
        XCTAssertFalse(client.requests.contains { $0.type == "GrantConnector" })
        XCTAssertTrue(session.error?.contains("changed") == true)
    }
}
