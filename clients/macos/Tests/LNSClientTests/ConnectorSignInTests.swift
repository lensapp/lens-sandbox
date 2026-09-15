import Foundation
import XCTest
@testable import LNSClient

final class ConnectorSignInTests: XCTestCase {
    func testNativeApprovalActionsAndRoundsMatchTheSharedRustContract() throws {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "sign-in", withExtension: "json", subdirectory: "Fixtures"))
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: [NSDictionary]])
        let actions: [ApprovalAction] = [.beginConnect(method: "token", label: "personal"),
            .answerConnect(values: ["otp": "fixture-secret", "account": "alice"]), .openConnectBrowser, .dismiss]
        let requests = try actions.map { try JSONSerialization.jsonObject(with: JSONEncoder().encode(ServiceRequest.respond(token: "round-2", action: $0))) as? NSDictionary }
        XCTAssertEqual(requests.compactMap { $0 }, fixture["requests"])
        let response = try XCTUnwrap(fixture["responses"]?.first)
        guard case let .snapshot(snapshot) = try ServiceReply.decode(JSONSerialization.data(withJSONObject: response)) else { return XCTFail("missing snapshot") }
        let approval = try XCTUnwrap(snapshot.approvals.first)
        XCTAssertEqual(approval.connect_seq, 2)
        XCTAssertEqual(approval.connect?.connector, "github")
        XCTAssertEqual(approval.connect?.fields.map(\.secret), [true, false])
        XCTAssertEqual(approval.connect?.from_code, true)
        XCTAssertEqual(approval.connect?.message, "Enter a one-time code")
        var method = try XCTUnwrap(approval.offer?.methods.first)
        XCTAssertEqual(method.hosts, ["https://provider.test"])
        XCTAssertEqual(method.codeDisclosure, "lns cannot show what this code does, and it runs programs on your machine with your own access. lns cannot bound what those reach.")
        method.runs_programs = false
        XCTAssertEqual(method.codeDisclosure, "lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has.")
        method.carries_code = false
        XCTAssertNil(method.codeDisclosure)
    }

    func testOAuthDisclosuresAndExplicitPermissionSelectionPreserveProviderDefaults() throws {
        let disclosure = try JSONDecoder().decode(OAuthDisclosure.self, from: Data(#"{"destinations":["https://provider.test/token"],"scope_options":[{"name":"default","label":"Default","scopes":[]}],"callback":"http://127.0.0.1/callback"}"#.utf8))
        XCTAssertEqual(disclosure.destinations, ["https://provider.test/token"])
        XCTAssertEqual(disclosure.callback, "http://127.0.0.1/callback")
        XCTAssertEqual(disclosure.scope_options.first?.permissions, "Provider default permissions")
        let progress = OAuthProgress.selectingScopes(disclosure.scope_options)
        var answers = ConnectAnswers()
        XCTAssertFalse(answers.ready(fields: [], progress: progress))
        answers.values["scopeOption"] = "default"
        XCTAssertTrue(answers.ready(fields: [], progress: progress))
        for progress in [OAuthProgress.starting(destinations: [], scopes: []), .deviceAuthorization(uri: "https://provider.test", code: "ABCD"), .waitingForBrowser(endpoint: "https://provider.test", redirect: "http://127.0.0.1"), .canceled, .expired] {
            XCTAssertFalse(answers.ready(fields: [], progress: progress))
        }
        XCTAssertThrowsError(try JSONDecoder().decode(OAuthProgress.self, from: Data(#"{"kind":"future"}"#.utf8)))
    }
}
