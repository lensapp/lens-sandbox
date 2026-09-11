import Foundation
import XCTest
@testable import LNSClient

@MainActor
final class SandboxCreationTests: XCTestCase {
    func testLocalLaunchUsesAnExplicitPathAndTheBundledCLIWithTheAppsSocket() async throws {
        var draft = SandboxDraft()
        draft.source = "/Users/person/My Project/lns.yaml"
        draft.name = "--debug"
        let launch = try HelperProcessLaunch(draft: draft, bundle: URL(fileURLWithPath: "/Applications/LNS.app"), socket: "/private/lns/service.sock", environment: ["LNS_HOME": "/data", "LNS_SOCKET_PATH": "/wrong"])
        XCTAssertEqual(launch.executable.path, "/Applications/LNS.app/Contents/Helpers/lns")
        XCTAssertEqual(launch.arguments, ["run", "--detach", "--name=--debug", "/Users/person/My Project/lns.yaml"])
        XCTAssertEqual(launch.environment["LNS_SOCKET_PATH"], "/private/lns/service.sock")
        XCTAssertEqual(launch.environment["LNS_HOME"], "/data")
        XCTAssertFalse(launch.arguments.contains("--yes"))
    }

    func testPublishedSetupRequiresAnExplicitChoiceAndInputsCannotBecomeFlags() async throws {
        var draft = SandboxDraft()
        draft.kind = .published; draft.source = "  ghcr.io/team/agent:latest  "
        XCTAssertEqual(try draft.arguments(), ["run", "--detach", "ghcr.io/team/agent:latest"])
        draft.allowSetup = true
        XCTAssertEqual(try draft.arguments(), ["run", "--detach", "--yes", "ghcr.io/team/agent:latest"])
        for invalid in ["", "--help", "a b", "repo\nother"] {
            draft.source = invalid
            XCTAssertThrowsError(try draft.arguments())
        }
        draft.kind = .local; draft.source = "./lns.yaml"
        XCTAssertThrowsError(try draft.arguments())
        draft.source = "/project/lns.yaml"
        for invalid in ["deadbeef", "name with spaces", "../name", "näme"] {
            draft.name = invalid
            XCTAssertThrowsError(try draft.arguments())
        }
    }

    func testLaunchWaitsForSuccessfulCLIExitAndReportsTheRunID() async {
        let creation = SandboxCreation { _ in
            AsyncThrowingStream { continuation in
                continuation.yield(.diagnostic(Data("Booting guest…\n".utf8)))
                continuation.yield(.output(Data("0123456789abcdef0123456789abcdef\n".utf8)))
                continuation.yield(.exited(0))
                continuation.finish()
            }
        }
        let started = await creation.start(localDraft())
        XCTAssertTrue(started)
        XCTAssertEqual(creation.runID, "0123456789abcdef0123456789abcdef")
        XCTAssertEqual(creation.output, "Booting guest…\n")
        XCTAssertFalse(creation.busy)
        XCTAssertNil(creation.error)
    }

    func testLaunchFailureKeepsTheDisclosureAndDoesNotRetryOrReportSuccess() async {
        var attempts = 0
        let creation = SandboxCreation { _ in
            attempts += 1
            return AsyncThrowingStream { continuation in
                continuation.yield(.diagnostic(Data("Host bind: /project → /workspace\nPermission required\n".utf8)))
                continuation.yield(.exited(125))
                continuation.finish()
            }
        }
        let started = await creation.start(localDraft())
        XCTAssertFalse(started)
        XCTAssertEqual(attempts, 1)
        XCTAssertNil(creation.runID)
        XCTAssertNotNil(creation.error)
        XCTAssertTrue(creation.output.contains("Host bind"))
    }

    func testLostExitOrMalformedOutputCannotClaimASandboxWasCreated() async {
        for events: [HelperProcessEvent] in [[.output(Data("0123456789abcdef0123456789abcdef\n".utf8))], [.exited(0)], [.output(Data("not a run id".utf8)), .exited(0)]] {
            let creation = SandboxCreation { _ in AsyncThrowingStream { continuation in
                events.forEach { continuation.yield($0) }; continuation.finish()
            } }
            let started = await creation.start(localDraft())
            XCTAssertFalse(started)
            XCTAssertNotNil(creation.error)
            XCTAssertNil(creation.runID)
        }
    }

    func testDiagnosticsAreBoundedAndReassembleSplitUTF8() async {
        let creation = SandboxCreation { _ in AsyncThrowingStream { continuation in
            continuation.yield(.diagnostic(Data(repeating: 97, count: 140_000)))
            continuation.yield(.diagnostic(Data([0xe2, 0x86])))
            continuation.yield(.diagnostic(Data([0x92])))
            continuation.yield(.exited(125))
            continuation.finish()
        } }
        _ = await creation.start(localDraft())
        XCTAssertTrue(creation.output.hasPrefix("[Earlier launch output omitted]\n"))
        XCTAssertTrue(creation.output.hasSuffix("→"))
        XCTAssertLessThan(creation.output.utf8.count, 132_000)
    }

    func testDuplicateLaunchIsRefusedWhileTheFirstIsPending() async {
        let stream = AsyncThrowingStream<HelperProcessEvent, Error>.makeStream()
        var launches = 0
        let creation = SandboxCreation { _ in launches += 1; return stream.stream }
        let first = Task { await creation.start(localDraft()) }
        while !creation.busy { await Task.yield() }
        let duplicate = await creation.start(localDraft())
        XCTAssertFalse(duplicate)
        XCTAssertEqual(launches, 1)
        stream.continuation.yield(.exited(125))
        stream.continuation.finish()
        _ = await first.value
        XCTAssertFalse(creation.busy)
    }

    private func localDraft() -> SandboxDraft {
        var draft = SandboxDraft(); draft.source = "/project/lns.yaml"
        return draft
    }
}
