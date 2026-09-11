import SwiftUI
import LNSClient

@MainActor
struct ApprovalOverlay: View {
    @ObservedObject var model: AppModel
    var maximumHeight: CGFloat = 620
    let hide: () -> Void
    let resize: (CGFloat) -> Void
    @State private var contentHeight: CGFloat = 240

    private var height: CGFloat { min(contentHeight + 49, maximumHeight) }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.shield").foregroundStyle(LNSTheme.accent)
                Text("LNS").font(.system(size: 13, weight: .semibold)).foregroundStyle(LNSTheme.heading)
                Text("·").foregroundStyle(LNSTheme.muted)
                Text("Access requests").font(.system(size: 12)).foregroundStyle(LNSTheme.muted)
                Spacer()
                if model.snapshot.approvals.count > 1 {
                    Text("\(model.snapshot.approvals.count)").font(.system(size: 11).monospacedDigit())
                        .foregroundStyle(LNSTheme.muted)
                }
                Button(action: hide) { Image(systemName: "xmark") }
                    .buttonStyle(.plain).foregroundStyle(LNSTheme.muted)
                    .frame(width: 24, height: 24)
                    .accessibilityLabel("Hide approval notifications")
                    .help("Hide notifications without answering requests")
            }
            .padding(.horizontal, 16).frame(height: 48)
            Rectangle().fill(LNSTheme.border).frame(height: 1)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let notice = model.notice { noticeView(notice) }
                    if let notice = model.connectionNotice { noticeView(notice) }
                    ForEach(Array(model.snapshot.notices.enumerated()), id: \.offset) { _, notice in
                        noticeView(notice)
                    }
                    if !model.snapshot.notices.isEmpty {
                        Button("Clear notices", action: model.dismissNotices)
                            .buttonStyle(.borderless).disabled(!model.connected)
                    }
                    ForEach(model.snapshot.approvals) { approval in
                        ApprovalCard(approval: approval, details: { model.reviewApproval(approval.id); hide() }) {
                            model.respond(to: approval, with: $0)
                        }
                            .disabled(!model.connected || approval.submitting || model.busy.contains(approval.id))
                    }
                    if model.snapshot.approvals.isEmpty && model.snapshot.notices.isEmpty && model.notice == nil {
                        LNSEmptyState(
                            symbol: "checkmark.shield",
                            title: model.connected ? "No requests waiting" : "Service disconnected",
                            message: model.connected ? "New access requests will appear here." : "Requests will appear when the service reconnects."
                        ).frame(maxWidth: .infinity)
                    }
                }
                .padding(16).frame(maxWidth: .infinity, alignment: .leading)
                .background {
                    GeometryReader { geometry in
                        Color.clear.preference(key: ApprovalContentHeight.self, value: geometry.size.height)
                    }
                }
            }
            .frame(height: max(1, height - 49))
        }
        .lnsAppearance()
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8).strokeBorder(LNSTheme.border, lineWidth: 1).allowsHitTesting(false)
        }
        .onPreferenceChange(ApprovalContentHeight.self) { value in
            guard value > 0 else { return }
            contentHeight = value
            resize(min(value + 49, maximumHeight))
        }
        .onChange(of: maximumHeight) { _ in resize(height) }
        .onExitCommand(perform: hide)
    }

    private func noticeView(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.triangle")
            .font(.system(size: 12)).foregroundStyle(LNSTheme.warning)
            .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12).background(LNSTheme.warning.opacity(0.08), in: RoundedRectangle(cornerRadius: 4))
    }
}

private struct ApprovalContentHeight: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
