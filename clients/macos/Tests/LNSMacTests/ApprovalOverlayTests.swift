import AppKit
import SwiftUI
import XCTest
@testable import LNSMac
import LNSClient

final class ApprovalOverlayTests: XCTestCase {
    @MainActor
    func testAuditDetailsFitTheNarrowDashboardContentWidth() async throws {
        let service = OverlayService()
        service.dashboardData.events = [try JSONDecoder().decode(DashboardEvent.self, from: Data(#"{"id":"event-1","ts":"2026-09-11T12:00:00Z","when":"12:00","run":"demo","kind":"egress","detail":"CONNECT example.com:443","raw":"{}"}"#.utf8))]
        let model = model(service).dashboard
        await model.refresh()
        model.selectedEvent = "event-1"
        let appeared = expectation(description: "Audit is laid out")
        let view = NSHostingView(rootView: AuditTimeline(model: model).onAppear { appeared.fulfill() })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 500),
                              styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = view
        defer { window.contentView = nil }
        view.layoutSubtreeIfNeeded()
        await fulfillment(of: [appeared], timeout: 2)
        let scrollViews = descendants(view).compactMap { $0 as? NSScrollView }
            .filter { !$0.isHiddenOrHasHiddenAncestor }
        XCTAssertFalse(scrollViews.isEmpty, "The test must lay out the actual audit content")
        for scrollView in scrollViews {
            let viewport = view.convert(scrollView.bounds, from: scrollView)
            XCTAssertGreaterThanOrEqual(viewport.minX, 0, "Audit content must not extend under the sidebar")
            XCTAssertLessThanOrEqual(viewport.maxX, 540, "Audit content must fit the narrow detail pane")
        }
    }

    @MainActor
    private func descendants(_ view: NSView) -> [NSView] {
        view.subviews.flatMap { [$0] + descendants($0) }
    }

    @MainActor
    func testOverlayReportsItsContentHeightInsteadOfKeepingTheInitialPanelHeight() async {
        let model = model()
        var measured: CGFloat = 0
        var changed = expectation(description: "Initial content is measured")
        let view = NSHostingView(rootView: ApprovalOverlay(model: model, hide: {}, resize: {
            measured = $0
            changed.fulfill()
        }))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 440, height: 289),
                              styleMask: .borderless, backing: .buffered, defer: false)
        window.contentView = view
        defer { window.contentView = nil }
        view.layoutSubtreeIfNeeded()
        await fulfillment(of: [changed], timeout: 2)
        XCTAssertGreaterThan(measured, 49, "The overlay must report its measured content height to the panel")
        XCTAssertLessThan(measured, 289, "A short empty state must shrink below the initial panel height")

        changed = expectation(description: "Long content expands to the scrolling limit")
        model.notice = Array(repeating: "A long fixture notice that must remain scrollable.", count: 80).joined(separator: "\n")
        await fulfillment(of: [changed], timeout: 2)
        XCTAssertEqual(measured, 620, "Long content must stop growing at the scrolling limit")

        changed = expectation(description: "Short content shrinks again")
        model.notice = nil
        await fulfillment(of: [changed], timeout: 2)
        XCTAssertLessThan(measured, 289, "Removing long content must not leave an empty panel")
    }

    @MainActor
    func testEscapeHidesPanelWithoutAnsweringRequests() async {
        let service = OverlayService()
        let model = model(service)
        let panel = ApprovalPanel(model: model)
        let received = expectation(description: "A request is waiting")
        model.onSnapshot = { snapshot in
            if !snapshot.approvals.isEmpty { received.fulfill() }
        }
        model.start()
        await fulfillment(of: [received], timeout: 2)
        defer { model.stop() }
        panel.show(focus: true)
        XCTAssertTrue(panel.isVisible)
        panel.cancelOperation(nil)
        XCTAssertFalse(panel.isVisible, "Escape must hide the panel even when no SwiftUI control owns focus")
        panel.show(focus: true)
        XCTAssertTrue(panel.isVisible, "Live Requests must be able to reopen the same panel")
        XCTAssertTrue(service.requests.isEmpty, "Hiding must not submit an approval action")
        XCTAssertEqual(model.snapshot.approvals.map(\.id), ["request-1"])
        panel.orderOut(nil)
    }

    @MainActor
    private func model(_ service: OverlayService = OverlayService()) -> AppModel {
        AppModel(service: service, socketPath: "/unused", launchService: nil, confirmStop: { false }, quit: {})
    }
}

private final class OverlayService: ServiceClient {
    var requests: [ServiceRequest] = []
    var dashboardData = DashboardData()
    func replies(to request: ServiceRequest, once: Bool, latestOnly: Bool) throws -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream {
            $0.yield(Data(#"{"type":"LiveApprovals","approvals":[{"id":"request-1","token":"token-1","host":"example.com","action":"CONNECT example.com:443","run":"demo","raw":false,"waiting":true,"submitting":false,"offer":null}],"notices":[]}"#.utf8))
        }
    }
    func send(_ request: ServiceRequest) async throws -> ServiceReply {
        requests.append(request)
        return .submitted
    }
    func dashboard() async throws -> DashboardData { dashboardData }
}
