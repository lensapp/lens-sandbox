import Foundation

public struct LiveConnectorDraft {
    public private(set) var selection = ""
    public var name = ""
    public var values: [String: String] = [:]
    public init() {}

    public mutating func choose(_ id: String) {
        selection = id
        name = ""
        values = [:]
    }

    public func newMethod(in offer: ConnectorOffer) -> ConnectorMethod? {
        offer.methods.first { $0.offerable && $0.auth_label != nil && selection == "new:\($0.name)" }
    }

    public func action(offer: ConnectorOffer) -> ApprovalAction? {
        if let option = offer.grantOptions.first(where: { $0.id == selection }) {
            return .grant(method: option.method, connection: option.connection.map { .held(label: $0) } ?? .none)
        }
        guard let method = newMethod(in: offer) else { return nil }
        let label = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty, !offer.connections.contains(where: { $0.label == label }),
              method.asks.allSatisfy({ !(values[$0] ?? "").isEmpty }) else { return nil }
        return .grant(method: method.name, connection: .new(label: label, values: values))
    }
}
