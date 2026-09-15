import Foundation

@MainActor
public final class DashboardRefresh {
    private let read: () async throws -> DashboardData
    private var task: Task<DashboardData, Error>?
    private var generation = 0
    private var dirty = false

    public init(read: @escaping () async throws -> DashboardData) { self.read = read }
    public func cancel() {
        generation += 1
        task?.cancel()
        task = nil
        dirty = false
    }

    public func refresh() async throws -> DashboardData {
        try Task.checkCancellation()
        let current = generation
        if let task {
            dirty = true
            let snapshot = try await task.value
            try Task.checkCancellation()
            guard generation == current else { throw CancellationError() }
            return snapshot
        }
        let pending = Task {
            defer { if generation == current { task = nil } }
            while true {
                dirty = false
                let snapshot = try await read()
                try Task.checkCancellation()
                if !dirty { return snapshot }
            }
        }
        task = pending
        let snapshot = try await withTaskCancellationHandler {
            try await pending.value
        } onCancel: { pending.cancel() }
        try Task.checkCancellation()
        guard generation == current else { throw CancellationError() }
        return snapshot
    }
}
