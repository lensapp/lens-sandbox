import Foundation

public struct LiveConnectorDraft {
    public private(set) var selection = ""
    public var name = ""
    public init() {}
    public init(offer: ConnectorOffer) {
        selection = offer.grantOptions.first?.id
            ?? offer.methods.first { $0.offerable && $0.auth_label != nil }.map { "new:\($0.name)" }
            ?? ""
    }

    public mutating func choose(_ id: String) {
        selection = id
        name = ""
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
        guard !label.isEmpty, !offer.connections.contains(where: { $0.label == label }) else { return nil }
        return .beginConnect(method: method.name, label: label)
    }
}
