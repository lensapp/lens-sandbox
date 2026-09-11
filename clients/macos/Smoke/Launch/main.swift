import Foundation
import LNSClient

@main
struct LaunchSmoke {
    @MainActor
    static func main() async throws {
        let bundle = URL(fileURLWithPath: CommandLine.arguments[1])
        var draft = SandboxDraft()
        draft.source = "/project with spaces/lns.yaml"
        draft.name = "--debug"
        for code in [0, 125] {
            let creation = SandboxCreation { draft in
                HelperProcess.launch(try HelperProcessLaunch(draft: draft, bundle: bundle,
                    socket: "/private/test/service.sock", environment: ["SMOKE_EXIT": String(code)]))
            }
            let success = await creation.start(draft)
            guard success == (code == 0), !creation.busy,
                  creation.output.hasSuffix("final launch diagnostic\n"),
                  creation.output.hasPrefix("[Earlier launch output omitted]\n"),
                  (code == 0 ? creation.runID == "0123456789abcdef0123456789abcdef" : creation.error != nil) else {
                throw ServiceError(message: "Launcher lost diagnostics or misreported exit \(code): \(creation.error ?? "no error")")
            }
        }
        print("PASS: bundled launch drains both pipes and preserves startup failure")
        let marker = bundle.appendingPathComponent("code-seen")
        let login = HelperProcessLaunch(arguments: ["login", "hub.lns.run"], bundle: bundle,
            socket: "/private/test/service.sock", environment: ["CODE_SEEN": marker.path])
        var sawCode = false
        var completed = false
        for try await event in HelperProcess.launch(login) {
            switch event {
            case let .output(bytes):
                if String(decoding: bytes, as: UTF8.self).contains("ABCD") {
                    sawCode = true
                    try Data().write(to: marker)
                }
            case let .exited(code):
                guard code == 0, sawCode else { throw ServiceError(message: "Browser code was buffered until the helper exited") }
                completed = true
            case .diagnostic: break
            }
        }
        guard completed else { throw ServiceError(message: "Browser helper did not finish") }
        print("PASS: browser confirmation code arrives while the helper is still waiting")
    }
}
