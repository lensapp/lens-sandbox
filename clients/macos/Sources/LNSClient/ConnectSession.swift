import Foundation

@MainActor
public final class ConnectSession {
    public private(set) var session: String?
    public private(set) var ask: ConnectAsk?
    public private(set) var progress: OAuthProgress?
    public private(set) var busy = false
    public private(set) var error: String?
    public private(set) var completed: String?
    public var onChange: (() -> Void)?
    private let service: any ServiceClient
    private let wait: () async throws -> Void
    private var generation = 0
    private var polling: Task<Void, Never>?

    public init(service: any ServiceClient, wait: @escaping () async throws -> Void = { try await Task.sleep(nanoseconds: 1_000_000_000) }) {
        self.service = service; self.wait = wait
    }

    public func begin(offer: ConnectorOffer, method: String, label: String) async {
        guard !busy, session == nil, completed == nil,
              offer.methods.contains(where: { $0.name == method && $0.offerable && $0.auth_label != nil }),
              !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !offer.connections.contains(where: { $0.label == label }) else { return }
        let current = generation
        busy = true; error = nil; onChange?()
        defer { if current == generation { busy = false; onChange?() } }
        do {
            guard case let .connectors(inventory) = try await service.manage(.listConnectors),
                  let installed = inventory.first(where: { $0.name == offer.name }), installed.digest == offer.digest,
                  installed.methods.contains(where: { $0.name == method && $0.offerable && $0.auth_label != nil }),
                  !installed.connections.contains(where: { $0.label == label }) else {
                throw ServiceError(message: "This connector changed. Close this form and review its current details before continuing.")
            }
            guard current == generation else { return }
            let reply = try await service.manage(.beginConnect(offer.name, method: method, label: label))
            await receive(reply, generation: current)
        } catch { if current == generation { self.error = error.localizedDescription } }
    }

    public func answer(_ values: [String: String]) async {
        var answers = ConnectAnswers(); answers.values = values
        guard let session, answers.ready(fields: ask?.fields ?? [], progress: progress) else { return }
        await execute(.answerConnect(session, values: values))
    }

    public func openBrowser() async {
        guard let session else { return }
        switch progress {
        case .deviceAuthorization, .waitingForBrowser: await execute(.openConnectBrowser(session))
        default: return
        }
    }

    public func checkStatus() async {
        guard let session, progress?.polls == true else { return }
        await execute(.connectStatus(session))
    }

    public func cancel() async {
        generation += 1
        polling?.cancel(); polling = nil
        let abandoned = session
        session = nil; ask = nil; progress = nil; busy = false; onChange?()
        guard let abandoned else { return }
        do { _ = try await service.manage(.cancelConnect(abandoned)) }
        catch { self.error = error.localizedDescription; onChange?() }
    }

    private func execute(_ command: ManagementCommand) async {
        guard !busy else { return }
        let current = generation
        busy = true; error = nil; onChange?()
        defer { if current == generation { busy = false; onChange?() } }
        do {
            let reply = try await service.manage(command)
            await receive(reply, generation: current)
        } catch { if current == generation { self.error = error.localizedDescription } }
    }

    private func receive(_ reply: ServiceReply, generation current: Int) async {
        guard current == generation else {
            let abandoned: String?
            switch reply {
            case let .connectAsk(ask): abandoned = ask.session
            case let .connectPending(session, _): abandoned = session
            default: abandoned = nil
            }
            if let abandoned {
                do { _ = try await service.manage(.cancelConnect(abandoned)) }
                catch { self.error = error.localizedDescription; onChange?() }
            }
            return
        }
        ask = nil; progress = nil
        switch reply {
        case let .connectAsk(asking): session = asking.session; ask = asking
        case let .connectPending(id, pending):
            session = id; progress = pending
            if pending == .canceled || pending == .expired { session = nil }
            startPolling()
        case let .completed(message): session = nil; completed = message
        case let .connectFailed(reason): session = nil; error = reason
        default: error = "The service did not return a sign-in result."
        }
    }

    private func startPolling() {
        guard polling == nil, progress?.polls == true else { return }
        polling = Task { [weak self] in
            guard let self else { return }
            defer { polling = nil }
            while progress?.polls == true, error == nil {
                do { try await wait(); try Task.checkCancellation() }
                catch { return }
                await checkStatus()
            }
        }
    }
}
