import Foundation

public struct ServiceLaunch {
    public let executable: URL
    public let arguments: [String]
    public let environment: [String: String]

    public init(bundle: URL, socket: String, environment: [String: String]) {
        let helpers = bundle.appendingPathComponent("Contents/Helpers", isDirectory: true)
        executable = helpers.appendingPathComponent("lns")
        arguments = ["service", "start"]
        var environment = environment
        environment["LNS_SERVICE_BIN"] = helpers.appendingPathComponent("lns-service").path
        environment["LNS_SOCKET_PATH"] = socket
        environment["LNS_HEADLESS"] = "1"
        self.environment = environment
    }
}
