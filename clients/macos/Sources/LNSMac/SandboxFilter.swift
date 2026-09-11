import SwiftUI

@MainActor
struct SandboxFilter: View {
    @ObservedObject var model: DashboardModel

    var body: some View {
        Menu {
            Picker("Sandbox", selection: Binding(
                get: { model.filters.sandbox },
                set: { model.selectSandbox($0) }
            )) {
                Text("All sandboxes").tag(String?.none)
                ForEach(model.data.sandboxes) { sandbox in
                    Text("\(sandbox.name) · \(sandbox.id)").tag(Optional(sandbox.id))
                }
            }
            .pickerStyle(.inline)
        } label: {
            Label(model.sandboxName, systemImage: "shippingbox").lineLimit(1)
        }
        .frame(maxWidth: 260)
        .accessibilityLabel("Filter by sandbox")
        .accessibilityValue(model.sandboxName)
        .help("Choose a sandbox or show all sandboxes")
    }
}
