import Foundation

public struct RegistryLogin: Decodable, Equatable, Identifiable {
    public let registry: String
    public let username: String
    public var id: String { registry }
}

public struct RegistryCommand: Encodable {
    public let type: String
    public var registry: String?
    public var username: String?
    public var secret: String?
    public static let list = Self(type: "ListRegistryLogins")
    public static func login(_ registry: String, username: String, secret: String) -> Self {
        Self(type: "RegistryLogin", registry: registry, username: username, secret: secret)
    }
    public static func logout(_ registry: String) -> Self { Self(type: "RegistryLogout", registry: registry) }
}

@MainActor
public final class RegistrySession {
    public private(set) var logins: [RegistryLogin] = []
    public private(set) var busy = false
    public private(set) var error: String?
    public private(set) var output = ""
    public private(set) var connected = false
    public var onChange: (() -> Void)?
    private let service: any ServiceClient
    private var generation = 0
    private var browserTask: Task<Bool, Never>?
    public init(service: any ServiceClient) { self.service = service }

    public static func host(_ value: String) throws -> String {
        let host = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !host.isEmpty, !host.hasPrefix("-"), !host.contains("/"),
              !host.contains(where: { $0.isWhitespace }),
              !host.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw ServiceError(message: "Enter a registry host such as hub.lns.run or ghcr.io, without a URL scheme or image path.")
        }
        return host
    }

    public func setConnected(_ value: Bool) {
        guard value != connected else { return }
        connected = value; generation += 1
        if !value { logins = []; browserTask?.cancel() }
        onChange?()
    }

    public func refresh() async {
        guard connected, !busy else { return }
        generation += 1
        let current = generation
        do { try await reload(current) }
        catch { if current == generation { self.error = error.localizedDescription; onChange?() } }
    }

    public func login(_ registry: String, username: String, secret: String) async -> Bool {
        await perform(.login(registry, username: username, secret: secret), expected: "RegistryLoginStored")
    }

    public func logout(_ registry: String) async -> Bool {
        await perform(.logout(registry), expected: "RegistryLoggedOut")
    }

    private func perform(_ submitted: RegistryCommand, expected: String) async -> Bool {
        guard connected, !busy else { return false }
        let current = begin()
        defer { busy = false; onChange?() }
        do {
            var command = submitted
            command.registry = try Self.host(command.registry ?? "")
            if command.type == "RegistryLogin" {
                guard !(command.username?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true),
                      !(command.secret?.isEmpty ?? true) else { throw ServiceError(message: "Enter a username and password or token.") }
            }
            guard case let .completed(type) = try await service.send(.registry(command)), type == expected else {
                throw ServiceError(message: "The service did not confirm the registry change.")
            }
            try await reload(current)
            return current == generation && connected
        } catch {
            if current == generation {
                let message = error.localizedDescription
                self.error = submitted.secret.flatMap { $0.isEmpty ? nil : message.replacingOccurrences(of: $0, with: "[redacted]") } ?? message
            }
            return false
        }
    }

    public func loginInBrowser(_ registry: String, launch: @escaping (String) throws -> AsyncThrowingStream<HelperProcessEvent, Error>) async -> Bool {
        guard connected, !busy else { return false }
        let current = begin()
        let task = Task { await consumeBrowser(registry, current: current, launch: launch) }
        browserTask = task
        defer { browserTask = nil; busy = false; onChange?() }
        return await withTaskCancellationHandler(operation: { await task.value }, onCancel: { task.cancel() })
    }

    public func cancelBrowser() { browserTask?.cancel() }

    private func consumeBrowser(_ registry: String, current: Int, launch: (String) throws -> AsyncThrowingStream<HelperProcessEvent, Error>) async -> Bool {
        var bytes = Data()
        do {
            try Task.checkCancellation()
            for try await event in try launch(Self.host(registry)) {
                try Task.checkCancellation()
                switch event {
                case let .output(chunk), let .diagnostic(chunk):
                    bytes.append(chunk)
                    if bytes.count > 65_536 { bytes = bytes.suffix(65_536) }
                    output = String(decoding: bytes, as: UTF8.self); onChange?()
                case let .exited(code):
                    guard code == 0 else { throw ServiceError(message: "Registry sign-in failed (exit \(code)). Review the details or use a username and token.") }
                    try await reload(current)
                    return current == generation && connected
                }
            }
            try Task.checkCancellation()
            throw ServiceError(message: "Sign-in ended without confirmation. Refresh the account list before trying again.")
        } catch {
            if current == generation { self.error = error is CancellationError ? "Sign-in canceled." : error.localizedDescription }
            return false
        }
    }

    private func begin() -> Int {
        busy = true; error = nil; output = ""; generation += 1; onChange?()
        return generation
    }

    private func reload(_ current: Int) async throws {
        guard connected, current == generation else { return }
        guard case let .registryLogins(logins) = try await service.send(.registry(.list)) else {
            throw ServiceError(message: "The service did not return registry accounts.")
        }
        guard connected, current == generation else { return }
        self.logins = logins.sorted { $0.registry < $1.registry }
        error = nil; onChange?()
    }
}
