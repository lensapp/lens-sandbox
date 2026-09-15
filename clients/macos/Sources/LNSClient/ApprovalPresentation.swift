public struct ApprovalPresentation {
    public enum Change { case show, hide, unchanged }
    private var presented: Set<String> = []
    private var notices: [String: Int] = [:]

    public init() {}

    public mutating func update(_ snapshot: ApprovalSnapshot) -> Change {
        let current = Set(snapshot.approvals.map(\.id))
        let counts = snapshot.notices.reduce(into: [String: Int]()) { $0[$1, default: 0] += 1 }
        defer { presented = current; notices = counts }
        if current.isEmpty && counts.isEmpty { return .hide }
        let newNotice = counts.contains { text, count in count > notices[text, default: 0] }
        return newNotice || !current.subtracting(presented).isEmpty ? .show : .unchanged
    }
}
