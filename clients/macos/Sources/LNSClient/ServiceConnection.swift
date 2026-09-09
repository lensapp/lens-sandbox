#if canImport(Network)
import Foundation
import Network

public struct ServiceConnection {
    public let path: String

    public init(path: String) { self.path = path }

    public func replies(to request: ServiceRequest, once: Bool = false) throws -> AsyncThrowingStream<Data, Error> {
        let frame = try FrameDecoder.encode(JSONEncoder().encode(request))
        let connection = NWConnection(to: .unix(path: path), using: .tcp)
        let queue = DispatchQueue(label: "run.lns.client.connection")
        return AsyncThrowingStream(bufferingPolicy: .bufferingNewest(1)) { continuation in
            var decoder = FrameDecoder()
            var started = false
            var received = false

            func finish(_ error: Error? = nil) {
                if let error { continuation.finish(throwing: error) }
                else { continuation.finish() }
                connection.cancel()
            }

            func receive() {
                connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, complete, error in
                    if let error { finish(error); return }
                    do {
                        for payload in try decoder.append(data ?? Data()) {
                            received = true
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

            continuation.onTermination = { _ in connection.cancel() }
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready where !started:
                    started = true
                    connection.send(content: frame, completion: .contentProcessed { error in
                        if let error { finish(error) }
                        else { receive() }
                    })
                case let .failed(error), let .waiting(error): finish(error)
                case .cancelled: continuation.finish()
                default: break
                }
            }
            queue.asyncAfter(deadline: .now() + 10) {
                if !received { finish(ServiceError(message: "The service did not respond.")) }
            }
            connection.start(queue: queue)
        }
    }

    public func send(_ request: ServiceRequest) async throws -> ServiceReply {
        for try await data in try replies(to: request, once: true) {
            return try ServiceReply.decode(data)
        }
        throw ServiceError(message: "The service disconnected before confirming the request. Check its state before trying again.")
    }
}
#endif
