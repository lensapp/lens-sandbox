import SwiftUI
import LNSClient

@MainActor
struct ApprovalHistory: View {
    @ObservedObject var model: DashboardModel
    private let answers = ["undecided", "withdrawn", "always allow", "always deny", "granted", "declined", "notice"]

    var body: some View {
        VStack(spacing: 0) {
            LNSPageHeader(title: "Approvals", subtitle: "Review access requests and saved decisions.") { EmptyView() }
            HStack(spacing: 12) {
                HStack(spacing: 8) {
                    SandboxFilter(model: model)
                    Menu {
                        Button("All answers") { model.filters.answers = []; model.clearHistory() }
                        Divider()
                        ForEach(answers, id: \.self) { answer in
                            Toggle(answer.capitalized, isOn: Binding(
                                get: { model.filters.answers.contains(answer) },
                                set: { on in
                                    if on { model.filters.answers.insert(answer) } else { model.filters.answers.remove(answer) }
                                    model.clearHistory()
                                }
                            ))
                        }
                    } label: { Label(model.filters.answers.isEmpty ? "All answers" : "\(model.filters.answers.count) answers", systemImage: "line.3.horizontal.decrease.circle") }
                }
                .fixedSize(horizontal: true, vertical: false).controlSize(.large)
                Spacer()
                LNSStatus(title: "\(model.waitingCount) waiting", color: model.waitingCount > 0 ? LNSTheme.warning : LNSTheme.muted)
            }
            .padding(.horizontal, 24).padding(.bottom, 16)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    groups(model.waiting)
                    if !model.archived.isEmpty {
                        DisclosureGroup("Archive (\(model.archived.count))", isExpanded: Binding(
                            get: { model.archiveChoice ?? model.waiting.isEmpty },
                            set: { model.archiveChoice = $0 }
                        )) {
                            groups(model.archived).padding(.top, 12)
                        }
                    }
                }
                .padding(.horizontal, 24).padding(.bottom, 24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .overlay {
                if model.history.isEmpty {
                    LNSEmptyState(
                        symbol: "checkmark.shield",
                        title: model.connected ? "No matching requests" : "Waiting for the service…",
                        message: "Requests for sandbox access and your answers appear here."
                    ).allowsHitTesting(false)
                }
            }
        }
    }

    private func groups(_ approvals: [DashboardApproval]) -> some View {
        let grouped = Dictionary(grouping: approvals) { $0.entry.sandbox ?? "No sandbox" }
        let names = approvals.reduce(into: [String]()) { names, approval in
            let name = approval.entry.sandbox ?? "No sandbox"
            if !names.contains(name) { names.append(name) }
        }
        return ForEach(names, id: \.self) { name in
            VStack(alignment: .leading, spacing: 8) {
                Text(name).font(.headline).foregroundStyle(LNSTheme.heading)
                ForEach(grouped[name] ?? []) { approval in
                    HistoryRow(approval: approval, model: model)
                }
            }
        }
    }
}

@MainActor
struct HistoryRow: View {
    let approval: DashboardApproval
    @ObservedObject var model: DashboardModel
    @State private var confirmingRemoval = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button { model.selectHistory(approval) } label: {
                HStack(spacing: 10) {
                    Image(systemName: model.selectedHistory == approval.id ? "chevron.down" : "chevron.right")
                        .font(.system(size: 10, weight: .semibold)).foregroundStyle(LNSTheme.muted).frame(width: 12)
                    Image(systemName: approval.entry.kind == "connector" ? "link" : (approval.entry.kind == "notice" ? "info.circle" : "network"))
                    Text(approval.entry.subject).font(.system(size: 13, weight: .medium)).foregroundStyle(LNSTheme.heading).lineLimit(2)
                    if approval.raw { Image(systemName: "eye.slash").help("LNS cannot inspect this traffic.") }
                    Spacer()
                    LNSStatus(title: approval.entry.answer, color: answerColor)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(approval.entry.subject), \(approval.entry.answer)")
            .accessibilityValue(model.selectedHistory == approval.id ? "Expanded" : "Collapsed")
            .accessibilityHint("Shows the question and available answers")
            if model.selectedHistory == approval.id {
                expansion
            }
        }
        .padding(16)
        .lnsPanel()
        .disabled(!model.connected || model.busy.contains(approval.id))
        .confirmationDialog("Remove this entry from the list? Its policy decision will remain in effect.", isPresented: $confirmingRemoval, titleVisibility: .visible) {
            Button("Remove from list", role: .destructive) { model.perform(.removeHistory(id: approval.id), for: approval.id) }
        }
    }

    private var answerColor: Color {
        switch approval.entry.answer {
        case "undecided", "withdrawn": return LNSTheme.warning
        case "always allow", "granted": return LNSTheme.success
        case "always deny", "declined": return LNSTheme.critical
        default: return LNSTheme.muted
        }
    }

    @ViewBuilder private var expansion: some View {
        if let action = approval.entry.action { Text(action).font(.system(.callout, design: .monospaced)).textSelection(.enabled) }
        if approval.raw { Label("LNS cannot inspect this traffic.", systemImage: "eye.slash").foregroundStyle(LNSTheme.warning) }
        if approval.grantable {
            if model.offerLoading { ProgressView("Loading current connector offer…").controlSize(.small) }
            if let error = model.offerError { Text(error).foregroundStyle(LNSTheme.warning).textSelection(.enabled) }
            if let offer = model.offer {
                ConnectorGrant(offer: offer, showsDecline: false) { action in
                    if case let .grant(method, connection) = action {
                        model.perform(.grantHistory(id: approval.id, method: method, digest: offer.digest, connection: connection), for: approval.id)
                    }
                }
                .id(approval.id + offer.digest)
            }
        }
        HStack {
            ForEach(approval.answers) { answer in
                Button(answer.label) { model.perform(.answerHistory(id: approval.id, answer: answer), for: approval.id) }
            }
            Spacer()
            Button(role: .destructive) { confirmingRemoval = true } label: {
                Label("Remove from list", systemImage: "trash")
            }
            .help("Only removes this entry. Its policy decision remains in effect.")
        }
        .buttonStyle(.bordered)
    }
}
