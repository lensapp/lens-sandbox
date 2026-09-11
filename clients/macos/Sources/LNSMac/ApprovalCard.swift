import SwiftUI
import LNSClient

struct ApprovalCard: View {
    let approval: LiveApproval
    var compact = true
    var details: (() -> Void)?
    let respond: (ApprovalAction) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Image(systemName: "shippingbox").foregroundStyle(LNSTheme.muted)
                Text(approval.run ?? "Sandbox").font(.system(size: 12, weight: .medium))
                    .foregroundStyle(LNSTheme.text).lineLimit(1).help(approval.run ?? "Sandbox")
                Spacer()
                Menu {
                    if approval.offer == nil {
                        Button("Allow Once") { respond(.allowOnce) }
                        Button("Deny Once") { respond(.denyOnce) }
                    } else {
                        Button("Skip Connector") { respond(.decline) }
                    }
                    Divider()
                    Button("Dismiss Request") { respond(.dismiss) }
                        .help("Fail this held request without recording a decision")
                } label: { Image(systemName: "ellipsis") }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).frame(width: 24)
                .accessibilityLabel("More actions for \(approval.host)")
            }
            VStack(alignment: .leading, spacing: 6) {
                Text(approval.offer == nil ? "Wants to connect to" : "Requests connector access")
                    .font(.system(size: 12)).foregroundStyle(LNSTheme.muted)
                Text(approval.offer?.name ?? approval.host)
                    .font(.system(size: 16, weight: .semibold)).foregroundStyle(LNSTheme.heading)
                    .lineLimit(2).help(approval.offer?.name ?? approval.host).textSelection(.enabled)
                if !compact {
                    Text(approval.action).font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(LNSTheme.text).textSelection(.enabled)
                    if approval.offer != nil {
                        Text(approval.host).font(.system(size: 12)).foregroundStyle(LNSTheme.muted).textSelection(.enabled)
                    }
                }
            }
            if approval.raw {
                Label("LNS cannot inspect this traffic.", systemImage: "eye.slash")
                    .font(.system(size: 12)).foregroundStyle(LNSTheme.warning)
            }
            if !approval.waiting {
                Text(approval.offer == nil ? "The request stopped waiting." : "The request stopped waiting. Connecting still applies to its next attempt.")
                    .font(.system(size: 12)).foregroundStyle(LNSTheme.muted)
            }
            if let offer = approval.offer {
                ConnectorGrant(offer: offer, showsDecline: false, showsDetails: !compact, details: details, respond: respond)
                    .id(offer.digest)
            } else {
                HStack {
                    if let details { Button("View details", action: details).buttonStyle(.borderless) }
                    Spacer(minLength: 8)
                    Button("Always Deny") { respond(.denyAlways) }.buttonStyle(LNSApprovalActionStyle())
                    Button("Always Allow") { respond(.allowAlways) }.buttonStyle(LNSApprovalActionStyle(prominent: true))
                }
            }
        }
        .padding(16).frame(maxWidth: .infinity, alignment: .leading).lnsPanel()
        .accessibilityElement(children: .contain)
    }
}
