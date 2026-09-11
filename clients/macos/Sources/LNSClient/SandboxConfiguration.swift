import Foundation

public struct ConfigurationSources: Codable, Equatable {
    public let definition: String
    public let mixins: [String]
    public let added_mixins: [String]
    public let contributions: [ConfigurationContribution]
}

public struct ConfigurationContribution: Codable, Equatable {
    public let block: String
    public let key: String
    public let source: String
    public let note: String?
    public let displaced: [ConfigurationDisplaced]?
}

public struct ConfigurationDisplaced: Codable, Equatable {
    public let source: String
    public let summary: String
}

public struct ConfigurationGrant: Decodable, Equatable {
    public let name: String
    public let variables: [String]
    public let files: [String]
}

public struct ConfigurationRule: Decodable, Equatable {
    public let table: String
    public let source: String
    public let rule: String
    public var fields: [String: Any] { SandboxConfiguration.object(rule) }
    public var destination: String { fields["match"] as? String ?? "Unknown destination" }
    public var verdict: String { fields["verdict"] as? String ?? "Unknown verdict" }
    public var description: String? { fields["description"] as? String }
    public func latestApproval(in events: [DashboardEvent], run: String) -> String? {
        guard source == "Your decision" else { return nil }
        return events.filter { event in
            guard event.run == run, event.kind == "approval",
                  let fields = SandboxConfiguration.object(event.raw)["unmapped"] as? [String: Any] else { return false }
            return fields["lns_target"] as? String == destination
                && fields["lns_decision"] as? String == "\(verdict)_always"
                && fields["lns_approval_kind"] as? String == "network"
        }.max { $0.ts < $1.ts }?.when
    }
}

public struct SandboxConfiguration: Decodable, Equatable {
    public let sources: ConfigurationSources?
    public let document: String
    public let decisions: String
    public let grants: [ConfigurationGrant]
    public let rules: [ConfigurationRule]
    public var spec: [String: Any] { Self.object(document)["spec"] as? [String: Any] ?? [:] }
    public var decisionSpec: [String: Any] { Self.object(decisions)["spec"] as? [String: Any] ?? [:] }
    public var userDecisions: [ConfigurationRule] { rules.filter { $0.source == "Your decision" } }
    public func overridingSource(for index: Int) -> String? {
        guard rules.indices.contains(index) else { return nil }
        let selected = rules[index]
        let scope = selected.fields.filter { $0.key != "verdict" && $0.key != "description" } as NSDictionary
        return rules.prefix(index).first {
            $0.table == selected.table && scope.isEqual(to: $0.fields.filter { $0.key != "verdict" && $0.key != "description" })
        }?.source
    }

    public static func object(_ text: String) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:]
    }
    public static func display(_ value: Any) -> String {
        if let text = value as? String { return text }
        guard JSONSerialization.isValidJSONObject(value),
              let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
              let text = String(data: data, encoding: .utf8) else { return String(describing: value) }
        return text
    }
}

extension SandboxConfiguration {
    private enum CodingKeys: String, CodingKey { case sources, document, decisions, grants, rules }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        sources = try values.decodeIfPresent(ConfigurationSources.self, forKey: .sources)
        document = try values.decode(String.self, forKey: .document)
        decisions = try values.decode(String.self, forKey: .decisions)
        grants = try values.decode([ConfigurationGrant].self, forKey: .grants)
        rules = try values.decode([ConfigurationRule].self, forKey: .rules)
        guard Self.object(document)["spec"] is [String: Any], Self.object(decisions)["spec"] is [String: Any],
              rules.allSatisfy({ $0.fields["match"] is String && ["allow", "deny"].contains($0.verdict) && ["http", "tcp"].contains($0.table) }) else {
            throw ServiceError(message: "The service returned an unreadable sandbox configuration.")
        }
    }
}

@MainActor
public final class ConfigurationSession {
    public private(set) var configuration: SandboxConfiguration?
    public private(set) var loading = false
    public private(set) var error: String?
    public var onChange: (() -> Void)?
    private let service: any ServiceClient
    private var generation = 0
    private var selection: [String] = []
    public init(service: any ServiceClient) { self.service = service }
    public func clear() {
        generation += 1; selection = []; configuration = nil; loading = false; error = nil; onChange?()
    }
    public func read(_ command: ManagementCommand) async {
        let selected = [command.type, command.run ?? "", command.source ?? ""] + (command.mixins ?? [])
        if selected != selection { configuration = nil }
        selection = selected
        generation += 1; error = nil; loading = true; onChange?()
        let current = generation
        do {
            let reply = try await service.send(.management(command))
            try Task.checkCancellation()
            guard current == generation else { return }
            guard case let .configuration(value) = reply else {
                throw ServiceError(message: "The service did not return the sandbox configuration.")
            }
            configuration = value
        } catch {
            guard current == generation else { return }
            configuration = nil; self.error = error.localizedDescription
        }
        loading = false; onChange?()
    }
}

@MainActor
public final class SandboxSaving {
    public private(set) var busy = false
    public private(set) var error: String?
    public var onChange: (() -> Void)?
    private let service: any ServiceClient
    private let write: (URL, Data) throws -> Void
    public init(service: any ServiceClient, write: @escaping (URL, Data) throws -> Void) {
        self.service = service; self.write = write
    }
    public func save(run: String, to file: URL) async -> Bool {
        guard !busy else { return false }
        busy = true; error = nil; onChange?()
        defer { busy = false; onChange?() }
        do {
            let name = file.deletingPathExtension().lastPathComponent
            guard file.isFileURL, name.utf8.count <= 63,
                  name.range(of: "^[a-z0-9]([a-z0-9-]*[a-z0-9])?$", options: .regularExpression) != nil else {
                throw ServiceError(message: "Use a filename with lowercase letters, digits, and dashes, starting and ending with a letter or digit (at most 63 characters).")
            }
            guard case let .savedDocument(document) = try await service.send(.management(.save(run, name: name))) else {
                throw ServiceError(message: "The service did not return a saved definition.")
            }
            try Task.checkCancellation()
            try write(file, Data(document.utf8))
            return true
        } catch { self.error = error.localizedDescription; return false }
    }
}
