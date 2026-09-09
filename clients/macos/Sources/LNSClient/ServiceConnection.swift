#if canImport(Network)
import Foundation
import Network

public struct ServiceConnection {
    public let path: String

    public init(path: String) { self.path = path }

    public func replies(to request: ServiceRequest, once: Bool = false, latestOnly: Bool = true) throws -> AsyncThrowingStream<Data, Error> {
        let frame = try FrameDecoder.encode(JSONEncoder().encode(request))
        let connection = NWConnection(to: .unix(path: path), using: .tcp)
        let queue = DispatchQueue(label: "run.lns.client.connection")
        return AsyncThrowingStream(bufferingPolicy: latestOnly ? .bufferingNewest(1) : .unbounded) { continuation in
            var decoder = FrameDecoder()
            var started = false
            var finished = false
            var deadline = ReplyDeadline(streaming: !once && latestOnly, now: ProcessInfo.processInfo.systemUptime)
            let timer = DispatchSource.makeTimerSource(queue: queue)

            func finish(_ error: Error? = nil) {
                guard !finished else { return }
                finished = true
                timer.cancel()
                connection.stateUpdateHandler = nil
                if let error { continuation.finish(throwing: error) }
                else { continuation.finish() }
                connection.cancel()
            }

            func receive() {
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, complete, error in
                    if let error { finish(error); return }
                    guard !finished else { return }
                    do {
                        for payload in try decoder.append(data ?? Data()) {
                            deadline.received(now: ProcessInfo.processInfo.systemUptime)
                            if !once && latestOnly { timer.cancel() }
                            continuation.yield(payload)
                            if once { finish(); return }
                        }
                        if complete {
                            try decoder.finish()
                            finish()
                        } else { receive() }
                    } catch { finish(error) }
                }
            }

            continuation.onTermination = { _ in queue.async { finish() } }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready where !started:
                    started = true
                    connection.send(content: frame, completion: .contentProcessed { error in
                        if let error { finish(error) }
                        else { receive() }
                    })
                case let .failed(error), let .waiting(error): finish(error)
                case .cancelled: finish()
                default: break
                }
            }
            timer.schedule(deadline: .now() + 1, repeating: 1)
            timer.setEventHandler {
                if deadline.expired(now: ProcessInfo.processInfo.systemUptime) {
                    finish(ServiceError(message: "The service stopped responding before completing the request. Check its state before trying again."))
                }
            }
            timer.resume()
            connection.start(queue: queue)
        }
    }

    public func send(_ request: ServiceRequest) async throws -> ServiceReply {
        for try await data in try replies(to: request, once: true) {
            return try ServiceReply.decode(data)
        }
        throw ServiceError(message: "The service disconnected before confirming the request. Check its state before trying again.")
    }

    public func dashboard() async throws -> DashboardData {
        var reader = DashboardRead()
        var snapshot: DashboardData?
        for try await data in try replies(to: .readDashboard, latestOnly: false) {
            try Task.checkCancellation()
            let message = try JSONDecoder().decode(DashboardMessage.self, from: data)
            if let complete = try reader.receive(message) { snapshot = complete }
        }
        try reader.finish()
        guard let snapshot else { throw ServiceError(message: "The service returned no dashboard snapshot.") }
        return snapshot
    }
}
#endif
