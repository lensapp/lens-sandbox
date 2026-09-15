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
    private var snapshot = Data()

    mutating func receive(_ data: Data?, complete: Bool, error: Error?, once: Bool, yield: (Data) -> Void) throws -> Bool {
        for payload in try decoder.append(data ?? Data()) {
            guard let reply = try assemble(payload) else { continue }
            yield(reply)
            if once { return true }
        }
        if let error { throw error }
        if complete {
            try decoder.finish()
            guard snapshot.isEmpty else { throw FrameError.truncated }
            return true
        }
        return false
    }

    private mutating func assemble(_ payload: Data) throws -> Data? {
        struct Envelope: Decodable { let type: String }
        let decoder = JSONDecoder()
        guard (try? decoder.decode(Envelope.self, from: payload).type) == "LiveApprovalsChunk" else {
            guard snapshot.isEmpty else { throw FrameError.truncated }
            return payload
        }
        struct Chunk: Decodable { let offset: Int; let data: String; let complete: Bool }
        let chunk = try decoder.decode(Chunk.self, from: payload)
        guard chunk.offset == snapshot.count, !chunk.data.isEmpty else { throw FrameError.truncated }
        snapshot.append(contentsOf: chunk.data.utf8)
        guard chunk.complete else { return nil }
        let result = snapshot
        snapshot = Data()
        return result
    }
}
