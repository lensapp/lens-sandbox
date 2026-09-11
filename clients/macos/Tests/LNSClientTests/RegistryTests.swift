import Foundation
import XCTest
@testable import LNSClient

@MainActor
final class RegistryTests: XCTestCase {
    final class Client: ServiceClient {
        var requests: [String] = []
        var response = "RegistryLoginStored"
        var holdList = false
        var listReply: CheckedContinuation<ServiceReply, Error>?
        var onList: (() -> Void)?
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> { throw ServiceError(message: "unexpected stream") }
        func dashboard() async throws -> DashboardData { throw ServiceError(message: "unexpected dashboard") }
        func send(_ request: ServiceRequest) async throws -> ServiceReply {
            requests.append(request.type)
            if request.type == "ListRegistryLogins" {
                if holdList { return try await withCheckedThrowingContinuation { listReply = $0; onList?() } }
                return try ServiceReply.decode(Data(#"{"type":"RegistryLogins","logins":[{"registry":"ghcr.io","username":"octocat"}]}"#.utf8))
            }
            if response == "error" { throw ServiceError(message: "rejected fixture-secret") }
            return try ServiceReply.decode(Data("{\"type\":\"\(response)\"}".utf8))
        }
    }

    func testRegistryWireTypesDecodeAndCredentialsUseTheSocketRequest() async throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "registry", withExtension: "json", subdirectory: "Fixtures"))
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: [NSDictionary]])
        let commands: [RegistryCommand] = [.login("ghcr.io", username: "octocat", secret: "fixture-secret"), .logout("ghcr.io"), .list]
        let requests = try commands.map { try JSONSerialization.jsonObject(with: JSONEncoder().encode(ServiceRequest.registry($0))) as? NSDictionary }
        XCTAssertEqual(requests.compactMap { $0 }, fixture["requests"])
        for response in try XCTUnwrap(fixture["responses"]) {
            _ = try ServiceReply.decode(JSONSerialization.data(withJSONObject: response))
        }
    }

    func testVerifiedLoginRefreshesTheSavedAccountsAndLogoutIsAcknowledged() async {
        let client = Client()
        let session = RegistrySession(service: client)
        session.setConnected(true)
        let success = await session.login("ghcr.io", username: "octocat", secret: "fixture-secret")
        XCTAssertTrue(success)
        XCTAssertEqual(session.logins.map(\.username), ["octocat"])
        XCTAssertEqual(client.requests, ["RegistryLogin", "ListRegistryLogins"])
        client.response = "RegistryLoggedOut"
        let removed = await session.logout("ghcr.io")
        XCTAssertTrue(removed)
        XCTAssertEqual(client.requests.suffix(2), ["RegistryLogout", "ListRegistryLogins"])
        session.setConnected(false)
        XCTAssertTrue(session.logins.isEmpty)
        XCTAssertFalse(session.busy)
    }

    func testRejectedLoginDoesNotRetryOrExposeTheSubmittedSecret() async {
        let client = Client(); client.response = "error"
        let session = RegistrySession(service: client); session.setConnected(true)
        let success = await session.login("ghcr.io", username: "octocat", secret: "fixture-secret")
        XCTAssertFalse(success)
        XCTAssertEqual(client.requests, ["RegistryLogin"])
        XCTAssertNotNil(session.error)
        XCTAssertFalse(session.error?.contains("fixture-secret") ?? true)
        XCTAssertTrue(session.output.isEmpty)
    }

    func testBrowserLoginShowsTheCodeAndNeedsASuccessfulExit() async {
        let client = Client()
        let session = RegistrySession(service: client); session.setConnected(true)
        let success = await session.loginInBrowser("hub.lns.run") { _ in AsyncThrowingStream { continuation in
            continuation.yield(.output(Data("Your code: ABCD\n".utf8)))
            continuation.yield(.exited(0)); continuation.finish()
        } }
        XCTAssertTrue(success)
        XCTAssertTrue(session.output.contains("ABCD"))
        XCTAssertEqual(client.requests, ["ListRegistryLogins"])
        let lost = await session.loginInBrowser("hub.lns.run") { _ in AsyncThrowingStream { $0.finish() } }
        XCTAssertFalse(lost)
        XCTAssertNotNil(session.error)
    }

    func testDisconnectDiscardsAnAccountListThatArrivesLate() async throws {
        let client = Client(); client.holdList = true
        let read = expectation(description: "list requested")
        client.onList = { read.fulfill() }
        let session = RegistrySession(service: client); session.setConnected(true)
        let task = Task { await session.refresh() }
        await fulfillment(of: [read], timeout: 1)
        session.setConnected(false)
        client.listReply?.resume(returning: try ServiceReply.decode(Data(#"{"type":"RegistryLogins","logins":[{"registry":"ghcr.io","username":"old"}]}"#.utf8)))
        await task.value
        XCTAssertTrue(session.logins.isEmpty)
    }

    func testBrowserCancellationStopsWaitingAndDuplicateSubmissionsDoNotLaunch() async {
        let client = Client()
        let session = RegistrySession(service: client); session.setConnected(true)
        let waiting = expectation(description: "browser waiting")
        let terminated = expectation(description: "stream terminated")
        let stream = AsyncThrowingStream<HelperProcessEvent, Error>.makeStream()
        stream.continuation.onTermination = { _ in terminated.fulfill() }
        let task = Task { await session.loginInBrowser("hub.lns.run") { _ in waiting.fulfill(); return stream.stream } }
        await fulfillment(of: [waiting], timeout: 1)
        let duplicate = await session.loginInBrowser("hub.lns.run") { _ in XCTFail("duplicate browser launch"); return stream.stream }
        XCTAssertFalse(duplicate)
        session.cancelBrowser()
        let success = await task.value
        XCTAssertFalse(success)
        XCTAssertFalse(session.busy)
        XCTAssertEqual(session.error, "Sign-in canceled.")
        XCTAssertTrue(client.requests.isEmpty)
        await fulfillment(of: [terminated], timeout: 1)
    }

    func testInvalidRegistryAndMissingCredentialsDoNotReachTheService() async {
        let client = Client()
        let session = RegistrySession(service: client); session.setConnected(true)
        for host in ["", "--help", "https://ghcr.io", "ghcr.io/org", "host name"] {
            let success = await session.login(host, username: "octocat", secret: "fixture-secret")
            XCTAssertFalse(success)
        }
        let empty = await session.login("ghcr.io", username: "", secret: "")
        XCTAssertFalse(empty)
        XCTAssertTrue(client.requests.isEmpty)
    }
}
