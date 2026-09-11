public struct ApprovalFeed {
    public private(set) var snapshot = ApprovalSnapshot.empty
    public private(set) var connected = false

    public init() {}

    public mutating func receive(_ snapshot: ApprovalSnapshot) {
        self.snapshot = snapshot
        connected = true
    }

    public mutating func disconnect() {
        connected = false
        snapshot = .empty
    }
}
