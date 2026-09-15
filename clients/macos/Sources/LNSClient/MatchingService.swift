import Foundation

public struct MatchingService: ServiceClient {
    public let client: any ServiceClient
    public let version: String

    public init(client: any ServiceClient, version: String) { self.client = client; self.version = version }

    private func checkVersion() async throws {
        struct Status: Decodable { let type: String; let version: String }
        let request = ServiceRequest.status
        for try await data in try client.replies(to: request, once: true, latestOnly: true) {
            let status = try JSONDecoder().decode(Status.self, from: data)
            guard status.type == "Status", status.version == version else {
                throw ServiceError(message: "LNS \(version) cannot use the running service \(status.version). Stop the service before opening this version of LNS; stopping interrupts running sandboxes.")
            }
            return
        }
        throw ServiceError(message: "The service did not report its version.")
    }

    public func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream(bufferingPolicy: latestOnly ? .bufferingNewest(1) : .unbounded) { continuation in
            let task = Task {
                do {
                    try await checkVersion()
                    try Task.checkCancellation()
                    for try await data in try client.replies(to: request, once: once, latestOnly: latestOnly) {
                        try Task.checkCancellation()
                        continuation.yield(data)
                    }
                    continuation.finish()
                } catch { continuation.finish(throwing: error) }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    public func send(_ request: ServiceRequest) async throws -> ServiceReply {
        try await checkVersion()
        try Task.checkCancellation()
        return try await client.send(request)
    }

    public func dashboard() async throws -> DashboardData {
        try await checkVersion()
        try Task.checkCancellation()
        return try await client.dashboard()
    }
}
