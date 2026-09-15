import Foundation
import XCTest
@testable import LNSClient

final class MatchingServiceTests: XCTestCase {
    final class Client: ServiceClient {
        var requests: [String] = []
        var version = "0.24.0"
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
            requests.append(request.type)
            return AsyncThrowingStream { continuation in
                let reply = request.type == "Status"
                    ? "{\"type\":\"Status\",\"version\":\"\(version)\",\"pid\":42,\"uptime_secs\":1}"
                    : "{\"type\":\"ShuttingDown\"}"
                continuation.yield(Data(reply.utf8))
                continuation.finish()
            }
        }
        func send(_ request: ServiceRequest) async throws -> ServiceReply {
            for try await data in try replies(to: request, once: true, latestOnly: true) { return try ServiceReply.decode(data) }
            throw ServiceError(message: "no reply")
        }
        func dashboard() async throws -> DashboardData { throw ServiceError(message: "unexpected dashboard") }
    }

    func testAnOlderServiceCannotReceiveAMutatingCommand() async {
        let client = Client()
        let service = MatchingService(client: client, version: "0.25.0")
        do {
            _ = try await service.send(.shutdown)
            XCTFail("an older service must be rejected before sending Shutdown")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("0.24.0"))
            XCTAssertTrue(error.localizedDescription.contains("0.25.0"))
        }
        XCTAssertEqual(client.requests, ["Status"])
    }

    func testEachCommandRechecksTheServiceAfterAnUpgrade() async throws {
        let client = Client()
        client.version = "0.25.0"
        let service = MatchingService(client: client, version: "0.25.0")
        guard case .shuttingDown = try await service.send(.shutdown) else { return XCTFail("matching service rejected") }
        client.version = "0.26.0"
        do {
            _ = try await service.send(.shutdown)
            XCTFail("the service version must not be cached")
        } catch {}
        XCTAssertEqual(client.requests, ["Status", "Shutdown", "Status"])
    }
}
