import XCTest
@testable import LNSClient

@MainActor
final class DashboardRefreshTests: XCTestCase {
    func testARefreshQueuedAtReadCompletionCannotReuseTheCompletedTask() async throws {
        var reads = 0
        var second: Task<DashboardData, Error>?
        var refresh: DashboardRefresh?
        defer { refresh = nil }
        refresh = DashboardRefresh {
            reads += 1
            var data = DashboardData()
            data.warnings = ["read \(reads)"]
            if reads == 1 {
                second = Task { try await XCTUnwrap(refresh).refresh() }
            }
            return data
        }
        _ = try await XCTUnwrap(refresh).refresh()
        let snapshot = try await XCTUnwrap(second).value
        XCTAssertEqual(snapshot.warnings, ["read 2"], "a refresh after a read completed needs a new read")
    }

    final class Reader {
        var pending: [CheckedContinuation<DashboardData, Error>] = []
        var onRead: (() -> Void)?
        func read() async throws -> DashboardData {
            try await withCheckedThrowingContinuation { continuation in
                pending.append(continuation)
                onRead?()
            }
        }
        func complete(_ warning: String) {
            var data = DashboardData()
            data.warnings = [warning]
            pending.removeFirst().resume(returning: data)
        }
    }

    func testDisconnectRefusesTheOldReadEvenIfTheTransportFinishesAfterCancellation() async throws {
        let reader = Reader()
        let started = expectation(description: "read started")
        reader.onRead = { started.fulfill() }
        let refresh = DashboardRefresh(read: reader.read)
        let old = Task { try await refresh.refresh() }
        await fulfillment(of: [started], timeout: 1)
        refresh.cancel()
        reader.complete("stale")
        do {
            _ = try await old.value
            XCTFail("a disconnected read must not restore actionable stale data")
        } catch is CancellationError {} catch { XCTFail("unexpected error: \(error)") }
    }

    func testAnActionDuringAReadRequestsAFreshSnapshotRatherThanReusingThePreActionRead() async throws {
        let reader = Reader()
        let started = expectation(description: "first read started")
        reader.onRead = { started.fulfill() }
        let refresh = DashboardRefresh(read: reader.read)
        let first = Task { try await refresh.refresh() }
        await fulfillment(of: [started], timeout: 1)
        let following = expectation(description: "follow-up read started")
        reader.onRead = { following.fulfill() }
        let secondStarted = expectation(description: "second refresh requested")
        let second = Task {
            secondStarted.fulfill()
            return try await refresh.refresh()
        }
        await fulfillment(of: [secondStarted], timeout: 1)
        reader.complete("before action")
        await fulfillment(of: [following], timeout: 1)
        reader.complete("after action")
        let snapshots = try await [first.value, second.value]
        XCTAssertEqual(snapshots.map(\.warnings), [["after action"], ["after action"]])
    }
}
