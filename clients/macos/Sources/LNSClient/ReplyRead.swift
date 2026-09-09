import Foundation

struct ReplyLifecycle {
    private(set) var started = false
    private(set) var finished = false
    var acceptsConnectionFailure: Bool { !started && !finished }

    mutating func start() -> Bool {
        guard !started && !finished else { return false }
        started = true
        return true
    }

    mutating func finish() -> Bool {
        guard !finished else { return false }
        finished = true
        return true
    }
}

struct ReplyRead {
    private var decoder = FrameDecoder()

    mutating func receive(_ data: Data?, complete: Bool, error: Error?, once: Bool, yield: (Data) -> Void) throws -> Bool {
        for payload in try decoder.append(data ?? Data()) {
            yield(payload)
            if once { return true }
        }
        if let error { throw error }
        if complete {
            try decoder.finish()
            return true
        }
        return false
    }
}
