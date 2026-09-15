import Foundation
import XCTest
@testable import LNSClient

@MainActor
final class ConnectSessionTests: XCTestCase {
    final class Client: ServiceClient {
        var commands: [NSDictionary] = []
        var responses: [String] = []
        var onRequest: (() -> Void)?
        var pending: AsyncThrowingStream<Data, Error>?
        func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
            commands.append(try JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as! NSDictionary)
            onRequest?()
            if let pending { self.pending = nil; return pending }
            let response = responses.removeFirst()
            return AsyncThrowingStream { $0.yield(Data(response.utf8)); $0.finish() }
        }
        func send(_ request: ServiceRequest) async throws -> ServiceReply { XCTFail("unexpected send"); return .acknowledged }
        func dashboard() async throws -> DashboardData { DashboardData() }
    }
    private let inventory = #"{"type":"ConnectorList","connectors":[{"name":"provider","digest":"one","serves":[],"methods":[{"name":"login","label":"Sign in","auth_label":"Token","offerable":true,"opens":[],"writes":[],"env":[],"credentials":[],"asks":["output"],"hosts":[],"runs_programs":false,"carries_code":false}],"connections":[]}]}"#
    private let first = #"{"type":"ConnectorAsks","session":"s1","message":"Account","from_code":true,"fields":[{"name":"account","label":"Account name","secret":false}]}"#
    private let second = #"{"type":"ConnectorAsks","session":"s1","message":"Code","from_code":true,"fields":[{"name":"otp","label":"One-time code","secret":true}]}"#
    private let connected = #"{"type":"ConnectorConnected","name":"provider","connection":"work","invalidated":["run-1"]}"#
    private let canceled = #"{"type":"ConnectorPending","session":"s1","progress":{"kind":"canceled"}}"#
    private func offer() throws -> ConnectorOffer {
        guard case let .connectors(offers) = try ServiceReply.decode(Data(inventory.utf8)) else { throw ServiceError(message: "fixture") }
        return try XCTUnwrap(offers.first)
    }
    func testSignInUsesServiceFieldsAcrossRoundsWithoutSendingOutputNames() async throws {
        let client = Client(); client.responses = [inventory, first, second, connected]
        let session = ConnectSession(service: client)
        await session.begin(offer: try offer(), method: "login", label: "work")
        XCTAssertEqual(session.ask?.fields.first?.name, "account")
        XCTAssertEqual(session.ask?.fields.first?.secret, false)
        await session.answer(["account": "alice"])
        XCTAssertEqual(session.ask?.fields.first?.name, "otp")
        await session.answer(["otp": "123456"])
        XCTAssertTrue(session.completed?.contains("run-1") == true)
        XCTAssertNil(session.session)
        XCTAssertEqual(client.commands.map { $0["type"] as? String }, ["ListConnectors", "BeginConnect", "AnswerConnect", "AnswerConnect"])
        XCTAssertNil(client.commands.dropFirst().first?["values"])
        XCTAssertEqual(client.commands.last?["values"] as? NSDictionary, ["otp": "123456"])
    }
    func testChangedOfferPreventsSignInBeforeAnyProviderAction() async throws {
        let client = Client(); client.responses = [inventory.replacingOccurrences(of: "\"one\"", with: "\"two\"")]
        let session = ConnectSession(service: client)
        await session.begin(offer: try offer(), method: "login", label: "work")
        XCTAssertNotNil(session.error)
        XCTAssertEqual(client.commands.count, 1)
    }
    func testCancelDiscardsTheRoundAndCancelsTheServiceSession() async throws {
        let client = Client(); client.responses = [inventory, first, canceled]
        let session = ConnectSession(service: client)
        await session.begin(offer: try offer(), method: "login", label: "work")
        await session.cancel()
        XCTAssertNil(session.ask)
        XCTAssertNil(session.session)
        XCTAssertNil(session.completed)
        XCTAssertEqual(client.commands.last?["type"] as? String, "CancelConnect")
        XCTAssertEqual(client.commands.last?["session"] as? String, "s1")
    }
    func testOAuthRequiresExplicitPresetThenPollsAutomatically() async throws {
        let client = Client()
        client.responses = [inventory,
            #"{"type":"ConnectorPending","session":"s1","progress":{"kind":"selecting_scopes","options":[{"name":"read","label":"Read only","scopes":[]}]}}"#,
            #"{"type":"ConnectorPending","session":"s1","progress":{"kind":"device_authorization","verification_uri":"https://provider.test/device","user_code":"ABCD"}}"#,
            connected]
        let polled = expectation(description: "automatic status poll")
        client.onRequest = { if client.commands.last?["type"] as? String == "ConnectStatus" { polled.fulfill() } }
        let session = ConnectSession(service: client, wait: {})
        await session.begin(offer: try offer(), method: "login", label: "work")
        XCTAssertEqual(client.commands.count, 2)
        var answers = ConnectAnswers()
        XCTAssertFalse(answers.ready(fields: [], progress: session.progress))
        await session.answer(["scopeOption": "not-offered"])
        XCTAssertEqual(client.commands.count, 2)
        answers.values = ["scopeOption": "read"]
        XCTAssertTrue(answers.ready(fields: [], progress: session.progress))
        await session.answer(answers.values)
        await fulfillment(of: [polled], timeout: 1)
        for _ in 0..<10 where session.completed == nil { await Task.yield() }
        XCTAssertNotNil(session.completed)
        XCTAssertEqual(client.commands.map { $0["type"] as? String }, ["ListConnectors", "BeginConnect", "AnswerConnect", "ConnectStatus"])
    }
    func testFailedMechanismEndsTheRoundAndCannotResubmitItsCredentials() async throws {
        let client = Client(); client.responses = [inventory, first,
            #"{"type":"ConnectorConnectFailed","name":"provider","reason":"Denied"}"#]
        let session = ConnectSession(service: client)
        await session.begin(offer: try offer(), method: "login", label: "work")
        await session.answer(["account": "alice"])
        XCTAssertNil(session.session)
        XCTAssertNil(session.ask)
        XCTAssertTrue(session.error?.contains("Denied") == true)
        XCTAssertNil(session.completed)
    }

    func testClosingDuringBeginCancelsTheLateSessionWithoutRestoringItsForm() async throws {
        let client = Client(); client.responses = [inventory, canceled]
        let began = expectation(description: "begin request")
        var reply: AsyncThrowingStream<Data, Error>.Continuation?
        client.onRequest = {
            if client.commands.last?["type"] as? String == "BeginConnect" {
                client.pending = AsyncThrowingStream { reply = $0 }
                began.fulfill()
            }
        }
        let session = ConnectSession(service: client)
        let offer = try offer()
        let beginning = Task { await session.begin(offer: offer, method: "login", label: "work") }
        await fulfillment(of: [began], timeout: 1)
        await session.cancel()
        reply?.yield(Data(first.utf8)); reply?.finish()
        await beginning.value
        XCTAssertNil(session.ask)
        XCTAssertNil(session.session)
        XCTAssertFalse(session.busy)
        XCTAssertEqual(client.commands.last?["type"] as? String, "CancelConnect")
    }

}
