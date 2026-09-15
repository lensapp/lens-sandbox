public struct ReplyDeadline {
    private let streaming: Bool
    private let timeout: Double
    private var until: Double?

    public init(streaming: Bool, now: Double, timeout: Double = 10) {
        self.streaming = streaming
        self.timeout = timeout
        until = now + timeout
    }

    public mutating func received(now: Double) { until = streaming ? nil : now + timeout }
    public func expired(now: Double) -> Bool { until.map { now >= $0 } ?? false }
}
