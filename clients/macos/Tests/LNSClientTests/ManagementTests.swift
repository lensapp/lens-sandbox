import Foundation
import XCTest
@testable import LNSClient

final class ManagementTests: XCTestCase {
    func testChoosingASavedConnectionDerivesItsMethodAndKeepsAccountFreeAccessSeparate() throws {
        let offer = ConnectorOffer(name: "github", digest: "one", serves: [], methods: [
            ConnectorMethod(name: "token", label: "Token", auth_label: "Token", offerable: true, opens: [], writes: [], env: [], credentials: [], asks: [], help: nil, overrides: nil),
            ConnectorMethod(name: "public", label: "Public access", auth_label: nil, offerable: true, opens: [], writes: [], env: [], credentials: [], asks: [], help: nil, overrides: nil),
            ConnectorMethod(name: "unsupported", label: "Unsupported", auth_label: "OAuth", offerable: false, opens: [], writes: [], env: [], credentials: [], asks: [], help: nil, overrides: nil)
        ], connections: [
            ConnectorConnection(label: "work", method: "token", authority: ["repo:read"]),
            ConnectorConnection(label: "public", method: "token", authority: []),
            ConnectorConnection(label: "old", method: "unsupported", authority: [])
        ])
        XCTAssertEqual(offer.grantOptions.map(\.id), ["connection:work", "connection:public", "method:public"])
        var selection = GrantSelection()
        selection.run = "run-1"
        selection.choose("connection:work", in: offer)
        XCTAssertEqual(selection.method, "token")
        XCTAssertEqual(selection.connection, "work")
        XCTAssertEqual(selection.run, "run-1")
        selection.choose("method:public", in: offer)
        XCTAssertEqual(selection.method, "public")
        XCTAssertEqual(selection.connection, "")
        selection.choose("connection:gone", in: offer)
        XCTAssertEqual(selection.method, "")
        XCTAssertEqual(selection.connection, "")
    }

    func testManagementCommandsAndResponsesMatchTheSharedRustFixture() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "management", withExtension: "json", subdirectory: "Fixtures"))
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: [NSDictionary]])
        let commands: [ManagementCommand] = [
            .start("run-1"), .stop("run-1"), .remove("run-1"), .listConnectors,
            .install("/recipes/github.yaml"), .uninstall("github"),
            .connect("github", method: "token", label: "work", values: ["token": "fixture-secret"]),
            .disconnect("github", connection: "work"),
            .grant("github", run: "run-1", method: "token", connection: "work"), .forget("github", run: "run-1")
        ]
        let requests = try commands.map { try JSONSerialization.jsonObject(with: JSONEncoder().encode(ServiceRequest.management($0))) as? NSDictionary }
        XCTAssertEqual(requests.compactMap { $0 }, fixture["requests"])
        for response in try XCTUnwrap(fixture["responses"]) {
            _ = try ServiceReply.decode(JSONSerialization.data(withJSONObject: response))
        }
    }

    func testLifecycleDeadlinesAllowTheServiceToFinishItsOwnStopAndBoot() {
        XCTAssertGreaterThan(ServiceRequest.management(.stop("run-1")).replyTimeout, 10)
        XCTAssertGreaterThanOrEqual(ServiceRequest.management(.start("run-1")).replyTimeout, 120)
        XCTAssertGreaterThanOrEqual(ServiceRequest.management(.install("registry/connector")).replyTimeout, 120)
        XCTAssertEqual(ServiceRequest.readDashboard.replyTimeout, 10)
    }
    func testCommandsTargetExplicitRunsAndKeepManagementConnectionsAsLabels() throws {
        let cases: [(ManagementCommand, NSDictionary)] = [
            (.start("run-1"), ["type": "StartRun", "run": "run-1", "attach": false, "stdin": false]),
            (.stop("run-1"), ["type": "StopRun", "run": "run-1", "timeout_secs": 10]),
            (.remove("run-1"), ["type": "RemoveRun", "run": "run-1", "force": false]),
            (.listConnectors, ["type": "ListConnectors"]),
            (.install("/recipes/github.yaml"), ["type": "InstallConnector", "source": "/recipes/github.yaml"]),
            (.uninstall("github"), ["type": "UninstallConnector", "name": "github"]),
            (.connect("github", method: "token", label: "work", values: ["token": "secret"]), ["type": "ConnectConnector", "name": "github", "method": "token", "connection": "work", "values": ["token": "secret"]]),
            (.disconnect("github", connection: "work"), ["type": "DisconnectConnector", "name": "github", "connection": "work"]),
            (.grant("github", run: "run-1", method: "token", connection: "work"), ["type": "GrantConnector", "name": "github", "run": "run-1", "method": "token", "connection": "work", "answered_by": "card"]),
            (.forget("github", run: "run-1"), ["type": "ForgetConnector", "name": "github", "run": "run-1"])
        ]
        for (command, expected) in cases {
            XCTAssertEqual(try JSONSerialization.jsonObject(with: JSONEncoder().encode(command)) as? NSDictionary, expected)
            XCTAssertEqual(try JSONSerialization.jsonObject(with: JSONEncoder().encode(ServiceRequest.management(command))) as? NSDictionary, expected)
        }
    }

    func testGrantRequiresAnExistingSandboxAndAnExplicitCompatibleConnection() throws {
        let offer = try JSONDecoder().decode(ConnectorOffer.self, from: Data(#"{"name":"github","digest":"sha256:one","serves":["api.github.com"],"methods":[{"name":"token","label":"Token","auth_label":"Token","offerable":true,"opens":["api.github.com"],"writes":[],"env":[],"credentials":["TOKEN"],"asks":["token"]},{"name":"public","label":"Public","offerable":true,"opens":[],"writes":[],"env":[],"credentials":[],"asks":[]},{"name":"unsupported","label":"Unsupported","offerable":false,"opens":[],"writes":[],"env":[],"credentials":[],"asks":[]}],"connections":[{"label":"work","method":"token","authority":["repo:read"]}]}"#.utf8))
        let sandboxes = [DashboardSandbox(id: "run-1", name: "agent", image: "alpine", status: "running"), DashboardSandbox(id: "gone", name: "old", image: "", status: "")]
        var selection = GrantSelection()
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes))
        selection.run = "run-1"; selection.method = "token"
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes), "choosing a method must not silently choose an account")
        selection.connection = "work"
        XCTAssertEqual(selection.command(offer: offer, sandboxes: sandboxes)?.connection, "work")
        selection.connection = "deleted"
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes))
        selection.method = "public"
        XCTAssertNotNil(selection.command(offer: offer, sandboxes: sandboxes))
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes)?.connection)
        selection.method = "unsupported"
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes))
        selection.method = "public"; selection.run = "gone"
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes), "audit-only sandbox rows cannot receive a grant")
        selection.run = "new-name"
        XCTAssertNil(selection.command(offer: offer, sandboxes: sandboxes), "this UI never silently reserves a future grant")
    }

    func testManagementRepliesPreserveActionOutcomes() throws {
        let cases = [
            (#"{"type":"RunStopped","forced":true}"#, "Sandbox stopped after forcing it to exit."),
            (#"{"type":"RunStarted","run_id":"run-1"}"#, "Sandbox started."),
            (#"{"type":"ConnectorGranted","name":"github","method":"token","connection":"work","displaced":"old","unchanged":false,"reserved":false}"#, "Access granted. Replaced old."),
            (#"{"type":"ConnectorGranted","name":"github","method":"token","connection":"work","displaced":null,"unchanged":true,"reserved":false}"#, "This sandbox already has that grant."),
            (#"{"type":"ConnectorForgotten","name":"github","had_decision":false,"reserved":false}"#, "This sandbox had no decision to forget.")
        ]
        for (json, expected) in cases {
            guard case let .completed(message) = try ServiceReply.decode(Data(json.utf8)) else {
                return XCTFail("management actions must report their outcome")
            }
            XCTAssertEqual(message, expected)
        }
    }

    func testConnectorInventoryUsesTheExistingOfferContract() throws {
        guard case let .connectors(offers) = try ServiceReply.decode(Data(#"{"type":"ConnectorList","connectors":[{"name":"github","digest":"sha256:one","serves":["api.github.com"],"methods":[],"connections":[]}]}"#.utf8)) else {
            return XCTFail("connector inventory must decode")
        }
        XCTAssertEqual(offers.first?.name, "github")
        XCTAssertEqual(offers.first?.digest, "sha256:one")
    }

    func testUnknownTargetsAreErrorsAndMalformedSuccessCannotSucceed() {
        for json in [#"{"type":"RunUnknown","run":"gone"}"#, #"{"type":"ConnectorUnknown","name":"gone"}"#, #"{"type":"RunStopped"}"#] {
            XCTAssertThrowsError(try ServiceReply.decode(Data(json.utf8)))
        }
    }
}
