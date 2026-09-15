import Foundation

public struct OAuthScopeOption: Decodable, Equatable, Identifiable {
    public let name: String
    public let label: String
    public let scopes: [String]
    public var id: String { name }
    public var permissions: String { scopes.isEmpty ? "Provider default permissions" : scopes.joined(separator: " ") }
}

public struct OAuthDisclosure: Decodable, Equatable {
    public let destinations: [String]
    public let scope_options: [OAuthScopeOption]
    public let callback: String?
}

public enum OAuthProgress: Decodable, Equatable {
    case selectingScopes([OAuthScopeOption])
    case starting(destinations: [String], scopes: [String])
    case deviceAuthorization(uri: String, code: String)
    case waitingForBrowser(endpoint: String, redirect: String)
    case canceled, expired

    private enum CodingKeys: String, CodingKey {
        case kind, options, destinations, scopes, verification_uri, user_code, authorization_endpoint, redirect_uri
    }
    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        switch try fields.decode(String.self, forKey: .kind) {
        case "selecting_scopes": self = .selectingScopes(try fields.decode([OAuthScopeOption].self, forKey: .options))
        case "starting": self = .starting(destinations: try fields.decode([String].self, forKey: .destinations), scopes: try fields.decode([String].self, forKey: .scopes))
        case "device_authorization": self = .deviceAuthorization(uri: try fields.decode(String.self, forKey: .verification_uri), code: try fields.decode(String.self, forKey: .user_code))
        case "waiting_for_browser": self = .waitingForBrowser(endpoint: try fields.decode(String.self, forKey: .authorization_endpoint), redirect: try fields.decode(String.self, forKey: .redirect_uri))
        case "canceled": self = .canceled
        case "expired": self = .expired
        default: throw ServiceError(message: "The service returned an unknown sign-in state. Update the app and service together.")
        }
    }
    public var polls: Bool {
        switch self {
        case .starting, .deviceAuthorization, .waitingForBrowser: return true
        default: return false
        }
    }
}

public struct ConnectorField: Decodable, Equatable, Identifiable {
    public let name: String
    public let label: String
    public let secret: Bool
    public var id: String { name }
}

public struct ConnectAsk: Decodable, Equatable {
    public let session: String
    public let message: String
    public let fields: [ConnectorField]
    public let from_code: Bool
}

public struct LiveConnectAsk: Decodable, Equatable {
    public let connector: String
    public let method: String
    public let message: String
    public let fields: [ConnectorField]
    public let from_code: Bool
    public let oauth: OAuthProgress?
}

public struct ConnectAnswers {
    public var values: [String: String] = [:]
    public init() {}
    public func ready(fields: [ConnectorField], progress: OAuthProgress?) -> Bool {
        if let progress {
            guard case let .selectingScopes(options) = progress else { return false }
            return options.contains { $0.name == values["scopeOption"] }
        }
        return fields.allSatisfy { !(values[$0.name] ?? "").isEmpty }
    }
}

extension ConnectorMethod {
    public var codeDisclosure: String? {
        guard carries_code else { return nil }
        return runs_programs
            ? "lns cannot show what this code does, and it runs programs on your machine with your own access. lns cannot bound what those reach."
            : "lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has."
    }
}
