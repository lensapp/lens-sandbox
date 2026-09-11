import Foundation

public protocol ServiceClient {
    func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error>
    func send(_ request: ServiceRequest) async throws -> ServiceReply
    func dashboard() async throws -> DashboardData
}
