#if canImport(Network)
import Foundation
import Network

public struct ServiceConnection: ServiceClient {
    public let path: String

    public init(path: String) { self.path = path }

    public func replies(to request: ServiceRequest, once: Bool = false, latestOnly: Bool = true) throws -> AsyncThrowingStream<Data, Error> {
        let frame = try FrameDecoder.encode(JSONEncoder().encode(request))
        let connection = NWConnection(to: .unix(path: path), using: .tcp)
        let queue = DispatchQueue(label: "run.lns.client.connection")
        return AsyncThrowingStream(bufferingPolicy: latestOnly ? .bufferingNewest(1) : .unbounded) { continuation in
            var reader = ReplyRead()
            var lifecycle = ReplyLifecycle()
            var deadline = ReplyDeadline(streaming: !once && latestOnly, now: ProcessInfo.processInfo.systemUptime, timeout: request.replyTimeout)
            let timer = DispatchSource.makeTimerSource(queue: queue)

            func finish(_ error: Error? = nil) {
                guard lifecycle.finish() else { return }
                timer.cancel()
                connection.stateUpdateHandler = nil
                if let error { continuation.finish(throwing: error) }
                else { continuation.finish() }
                connection.cancel()
            }

            func receive() {
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, complete, error in
                    guard !lifecycle.finished else { return }
                    do {
                        let ended = try reader.receive(data, complete: complete, error: error, once: once) { payload in
                            deadline.received(now: ProcessInfo.processInfo.systemUptime)
                            if !once && latestOnly { timer.cancel() }
                            continuation.yield(payload)
                        }
                        if ended { finish() }
                        else { receive() }
                    } catch { finish(error) }
                }
            }

            continuation.onTermination = { _ in queue.async { finish() } }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready where lifecycle.start():
                    connection.send(content: frame, completion: .contentProcessed { error in
                        if let error { finish(error) }
                        else { receive() }
                    })
                case let .failed(error), let .waiting(error):
                    if lifecycle.acceptsConnectionFailure { finish(error) }
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
        try await readDashboard(replies(to: .readDashboard, latestOnly: false))
    }
}
#endif
