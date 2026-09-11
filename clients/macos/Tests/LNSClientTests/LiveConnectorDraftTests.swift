import Foundation
import XCTest
@testable import LNSClient

final class LiveConnectorDraftTests: XCTestCase {
    private func offer(available: Bool = true) -> ConnectorOffer {
        ConnectorOffer(name: "github", digest: "one", serves: ["api.github.com"], methods: [
            ConnectorMethod(name: "token", label: "Token", auth_label: "Token", offerable: available,
                opens: ["api.github.com"], writes: [], env: [], credentials: ["TOKEN"], asks: ["token"], help: nil, overrides: []),
            ConnectorMethod(name: "public", label: "Public", auth_label: nil, offerable: true,
                opens: [], writes: [], env: [], credentials: [], asks: [], help: nil, overrides: [])
        ], connections: [ConnectorConnection(label: "work", method: "token", authority: ["repo:read"])])
    }

    private func encoded(_ draft: LiveConnectorDraft) throws -> NSDictionary {
        let action = try XCTUnwrap(draft.action(offer: offer()))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(action)) as? NSDictionary)
    }

    func testSavedConnectionDeterminesTheMethodWithoutSupplyingCredentials() throws {
        var draft = LiveConnectorDraft()
        XCTAssertNil(draft.action(offer: offer()), "a notification must require an explicit choice")
        draft.choose("connection:work")
        XCTAssertEqual(try encoded(draft), ["kind": "grant", "method": "token", "connection": ["kind": "held", "label": "work"]])
        draft.choose("method:public")
        XCTAssertEqual(try encoded(draft), ["kind": "grant", "method": "public", "connection": ["kind": "none"]])
    }

    func testNewConnectionRequiresCompleteCredentialsAndANewName() throws {
        var draft = LiveConnectorDraft()
        draft.choose("new:token")
        XCTAssertEqual(draft.newMethod(in: offer())?.name, "token")
        draft.name = "personal"
        XCTAssertNil(draft.action(offer: offer()))
        draft.values["token"] = "fixture-secret"
        draft.name = " work "
        XCTAssertNil(draft.action(offer: offer()), "the inline form must not replace a saved account")
        draft.name = "  "
        XCTAssertNil(draft.action(offer: offer()))
        draft.name = " personal "
        XCTAssertEqual(try encoded(draft), ["kind": "grant", "method": "token", "connection": [
            "kind": "new", "label": "personal", "values": ["token": "fixture-secret"]
        ]])
    }

    func testSwitchingConnectionsClearsUnsubmittedCredentials() {
        var draft = LiveConnectorDraft()
        draft.choose("new:token")
        draft.name = "personal"
        draft.values["token"] = "fixture-secret"
        draft.choose("connection:work")
        XCTAssertEqual(draft.name, "")
        XCTAssertEqual(draft.values, [:], "secrets from a new connection must not follow another selection")
    }

    func testRemovedConnectionsAndUnavailableMethodsCannotBeGranted() {
        var draft = LiveConnectorDraft()
        for selection in ["connection:gone", "new:missing", "new:public", "method:token"] {
            draft.choose(selection)
            draft.name = "personal"; draft.values["token"] = "fixture-secret"
            XCTAssertNil(draft.action(offer: offer()))
        }
        for selection in ["connection:work", "new:token"] {
            draft.choose(selection)
            draft.name = "personal"; draft.values["token"] = "fixture-secret"
            XCTAssertNil(draft.action(offer: offer(available: false)))
        }
    }
}
