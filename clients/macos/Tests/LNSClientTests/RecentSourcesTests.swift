import XCTest
@testable import LNSClient

final class RecentSourcesTests: XCTestCase {
    func testRecentsRememberSourcesWithoutReapplyingAPreviousComposition() throws {
        var recent = RecentSources()
        var draft = SandboxDraft(); draft.source = "/project/lns.yaml"
        draft.mixins = ["/tools/lns.yaml", "ghcr.io/team/network:latest"]
        recent.record(draft)
        XCTAssertEqual(recent.definitions, [draft.source])
        XCTAssertEqual(recent.mixins, draft.mixins)
        recent.saved("/saved/reviewer.yaml")
        XCTAssertEqual(recent.definitions, ["/saved/reviewer.yaml", draft.source])
        let restored = try JSONDecoder().decode(RecentSources.self, from: JSONEncoder().encode(recent))
        XCTAssertEqual(restored, recent)
        recent.removeMixin(draft.mixins[0]); recent.removeDefinition(draft.source)
        XCTAssertEqual(recent.mixins, [draft.mixins[1]])
        XCTAssertEqual(recent.definitions, ["/saved/reviewer.yaml"])
    }

    func testRecentsAreDeduplicatedAndBounded() {
        var recent = RecentSources()
        for index in 0..<30 { recent.saved("/saved/\(index).yaml") }
        recent.saved("/saved/20.yaml")
        XCTAssertEqual(recent.definitions.count, 12)
        XCTAssertEqual(recent.definitions.first, "/saved/20.yaml")
        XCTAssertEqual(Set(recent.definitions).count, 12)
    }
}
