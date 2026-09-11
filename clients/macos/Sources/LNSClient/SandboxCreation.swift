import Foundation

public struct SandboxDraft {
    public enum Source: String, CaseIterable { case local, published }
    public var kind = Source.local
    public var source = ""
    public var name = ""
    public var allowSetup = false
    public init() {}

    public func arguments() throws -> [String] {
        let source = source.trimmingCharacters(in: .whitespacesAndNewlines)
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !source.isEmpty, !source.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw ServiceError(message: "Choose a sandbox definition or enter a published reference.")
        }
        if kind == .local {
            guard source.hasPrefix("/") else { throw ServiceError(message: "Choose a local file or folder using its full path.") }
        } else {
            guard !source.hasPrefix("-"), !source.hasPrefix("/"), !source.contains(where: \.isWhitespace) else {
                throw ServiceError(message: "Enter a published sandbox reference, such as ghcr.io/team/agent:latest.")
            }
        }
        if !name.isEmpty {
            let allowed = name.utf8.allSatisfy { (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || [45, 46, 95].contains($0) }
            guard allowed, !name.utf8.allSatisfy(Self.isLowerHex) else {
                throw ServiceError(message: "Use letters, numbers, dots, dashes, or underscores for the name; it cannot consist entirely of lowercase hexadecimal characters.")
            }
        }
        var arguments = ["run", "--detach"]
        if !name.isEmpty { arguments.append("--name=\(name)") }
        if allowSetup { arguments.append("--yes") }
        arguments.append(source)
        return arguments
    }

    static func isLowerHex(_ byte: UInt8) -> Bool { (48...57).contains(byte) || (97...102).contains(byte) }
}

public struct HelperProcessLaunch {
    public let executable: URL
    public let arguments: [String]
    public let environment: [String: String]

    public init(draft: SandboxDraft, bundle: URL, socket: String, environment: [String: String]) throws {
        self.init(arguments: try draft.arguments(), bundle: bundle, socket: socket, environment: environment)
    }

    public init(arguments: [String], bundle: URL, socket: String, environment: [String: String]) {
        let helper = ServiceLaunch(bundle: bundle, socket: socket, environment: environment)
        executable = helper.executable
        self.arguments = arguments
        self.environment = helper.environment
    }
}

public enum HelperProcessEvent {
    case output(Data), diagnostic(Data), exited(Int32)
}

@MainActor
public final class SandboxCreation {
    public private(set) var busy = false
    public private(set) var output = ""
    public private(set) var error: String?
    public private(set) var runID: String?
    public var onChange: (() -> Void)?
    private let launch: (SandboxDraft) throws -> AsyncThrowingStream<HelperProcessEvent, Error>

    public init(launch: @escaping (SandboxDraft) throws -> AsyncThrowingStream<HelperProcessEvent, Error>) { self.launch = launch }
    public func start(_ draft: SandboxDraft) async -> Bool {
        guard !busy else { return false }
        busy = true; output = ""; error = nil; runID = nil; onChange?()
        defer { busy = false; onChange?() }
        var identifier = Data()
        var diagnostics = Data()
        var truncated = false
        do {
            _ = try draft.arguments()
            for try await event in try launch(draft) {
                try Task.checkCancellation()
                switch event {
                case let .output(bytes):
                    guard identifier.count + bytes.count <= 1024 else { throw ServiceError(message: "The launcher returned unexpected output. Refresh the sandbox list before trying again.") }
                    identifier.append(bytes)
                case let .diagnostic(bytes):
                    diagnostics.append(bytes)
                    if diagnostics.count > 131_072 { diagnostics = diagnostics.suffix(131_072); truncated = true }
                    output = (truncated ? "[Earlier launch output omitted]\n" : "") + String(decoding: diagnostics, as: UTF8.self)
                    onChange?()
                case let .exited(code):
                    guard code == 0 else { throw ServiceError(message: "The sandbox did not finish starting (exit \(code)). Review the launch details below before trying again.") }
                    let id = String(decoding: identifier, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                    guard id.utf8.count == 32, id.utf8.allSatisfy(SandboxDraft.isLowerHex) else {
                        throw ServiceError(message: "The launcher did not return a sandbox ID. Refresh the sandbox list before trying again.")
                    }
                    runID = id
                    return true
                }
            }
            throw ServiceError(message: "The launcher disconnected before confirming startup. Refresh the sandbox list before trying again.")
        } catch {
            self.error = error.localizedDescription
            return false
        }
    }
}
