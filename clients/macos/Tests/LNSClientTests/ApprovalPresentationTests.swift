import Foundation
import XCTest
@testable import LNSClient

final class ApprovalPresentationTests: XCTestCase {
    func snapshot(_ notices: [String], approval: Bool = false) throws -> ApprovalSnapshot {
        let row: [String: Any] = ["id": "request", "token": "token", "host": "example.com", "action": "CONNECT", "raw": false, "waiting": true, "submitting": false]
        return try JSONDecoder().decode(ApprovalSnapshot.self, from: JSONSerialization.data(withJSONObject: ["approvals": approval ? [row] : [], "notices": notices]))
    }

    func testPersistenceFailureShowsAfterTheLastRequestDisappears() throws {
        var presentation = ApprovalPresentation()
        XCTAssertEqual(presentation.update(try snapshot([], approval: true)), .show)
        XCTAssertEqual(presentation.update(.empty), .hide)
        XCTAssertEqual(presentation.update(try snapshot(["Could not save the decision"])), .show,
                       "a failure notice must reopen the panel after its request disappears")
    }

    func testAnExistingNoticeKeepsThePanelUpWithoutRepeatedlyRaisingIt() throws {
        var presentation = ApprovalPresentation()
        _ = presentation.update(try snapshot(["Could not save"], approval: true))
        XCTAssertEqual(presentation.update(try snapshot(["Could not save"])), .unchanged)
        XCTAssertEqual(presentation.update(try snapshot(["Could not save"])), .unchanged)
        XCTAssertEqual(presentation.update(.empty), .hide)
    }

    func testNewAndRepeatedFailureNoticesRaiseAHiddenPanel() throws {
        var presentation = ApprovalPresentation()
        _ = presentation.update(try snapshot(["Failure"], approval: true))
        XCTAssertEqual(presentation.update(try snapshot(["Failure", "Failure"], approval: true)), .show)
        XCTAssertEqual(presentation.update(try snapshot(["Failure", "Failure"], approval: true)), .unchanged)
        XCTAssertEqual(presentation.update(try snapshot(["Failure", "Another failure"], approval: true)), .show)
    }
}
