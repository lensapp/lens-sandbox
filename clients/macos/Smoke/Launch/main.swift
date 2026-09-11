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
                SandboxProcess.launch(try SandboxProcessLaunch(draft: draft, bundle: bundle,
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
    }
}
