import Foundation

public struct FrameDecoder {
    private var buffer = Data()
    private static let magic = Data([76, 78, 83, 50])
    private static let maximumLength = 1_048_576

    public init() {}

    public mutating func append(_ bytes: Data) throws -> [Data] {
        buffer.append(bytes)
        var frames: [Data] = []
        while buffer.count >= 8 {
            guard buffer.prefix(4) == Self.magic else { throw FrameError.badMagic }
            let length = buffer.dropFirst(4).prefix(4).reduce(0) { ($0 << 8) | Int($1) }
            guard (1...Self.maximumLength).contains(length) else { throw FrameError.invalidLength }
            guard buffer.count >= 9 else { break }
            guard buffer[buffer.startIndex + 8] == 1 else { throw FrameError.unexpectedSubtype }
            guard buffer.count >= 8 + length else { break }
            frames.append(Data(buffer.dropFirst(9).prefix(length - 1)))
            buffer.removeFirst(8 + length)
        }
        return frames
    }

    public func finish() throws {
        guard buffer.isEmpty else { throw FrameError.truncated }
    }

    public static func encode(_ payload: Data) throws -> Data {
        guard payload.count < maximumLength else { throw FrameError.invalidLength }
        let length = UInt32(payload.count + 1)
        let header = [24, 16, 8, 0].map { UInt8(truncatingIfNeeded: length >> $0) }
        return magic + Data(header) + Data([1]) + payload
    }
}

public enum FrameError: Error {
    case badMagic, invalidLength, unexpectedSubtype, truncated
}
