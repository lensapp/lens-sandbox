import Foundation

public struct RecentSources: Codable, Equatable {
    public private(set) var definitions: [String] = []
    public private(set) var mixins: [String] = []
    public init() {}
    public mutating func record(_ draft: SandboxDraft) {
        saved(draft.source.trimmingCharacters(in: .whitespacesAndNewlines))
        for source in draft.mixins.reversed() { Self.remember(source, in: &mixins) }
    }
    public mutating func saved(_ path: String) { Self.remember(path, in: &definitions) }
    public mutating func removeDefinition(_ source: String) { definitions.removeAll { $0 == source } }
    public mutating func removeMixin(_ source: String) { mixins.removeAll { $0 == source } }
    private static func remember(_ source: String, in items: inout [String]) {
        items.removeAll { $0 == source }
        items.insert(source, at: 0)
        items = Array(items.prefix(12))
    }
}
