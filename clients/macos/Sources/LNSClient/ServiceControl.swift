import Foundation

@MainActor
public final class ServiceControl {
    public enum Phase { case idle, starting, stopping }
    public private(set) var phase = Phase.idle
    public var onChange: (() -> Void)?
    public var onError: ((String) -> Void)?
    private let client: any ServiceClient
    private let launch: (() async throws -> Void)?
    private let confirm: () -> Bool
    private let quit: () -> Void
    private var attemptedStartup = false

    public init(client: any ServiceClient, launch: (() async throws -> Void)?, confirm: @escaping () -> Bool, quit: @escaping () -> Void) {
        self.client = client; self.launch = launch; self.confirm = confirm; self.quit = quit
    }
    public var canStart: Bool { launch != nil && phase == .idle }
    public func startOnLaunch() async {
        guard !attemptedStartup else { return }
        attemptedStartup = true
        await start()
    }
    public func start() async {
        guard canStart, let launch else { return }
        setPhase(.starting)
        defer { setPhase(.idle) }
        do { try await launch() }
        catch { onError?(error.localizedDescription) }
    }

    public func stop(connected: Bool) async {
        guard connected, phase == .idle else { return }
        setPhase(.stopping)
        defer { setPhase(.idle) }
        guard confirm() else { return }
        do {
            guard case .shuttingDown = try await client.send(.shutdown) else {
                throw ServiceError(message: "The service did not confirm shutdown.")
            }
            quit()
        } catch { onError?(error.localizedDescription) }
    }

    private func setPhase(_ phase: Phase) { self.phase = phase; onChange?() }
}
