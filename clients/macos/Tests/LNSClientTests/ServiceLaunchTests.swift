import Foundation
import XCTest
@testable import LNSClient

final class ServiceLaunchTests: XCTestCase {
    func testStartUsesOnlyTheBundledHelpersAndDoesNotRegisterALoginAgent() {
        let launch = ServiceLaunch(bundle: URL(fileURLWithPath: "/Applications/My Apps/LNS.app"), socket: "/private/test/service.sock", environment: [
            "PATH": "/untrusted/bin", "LNS_SERVICE_BIN": "/old/lns-service", "LNS_HEADLESS": "0",
            "LNS_HOME": "/isolated/data", "KEEP_ME": "value"
        ])
        XCTAssertEqual(launch.executable.path, "/Applications/My Apps/LNS.app/Contents/Helpers/lns")
        XCTAssertEqual(launch.arguments, ["service", "start"], "starting must not silently enable login launch")
        XCTAssertEqual(launch.environment["LNS_SERVICE_BIN"], "/Applications/My Apps/LNS.app/Contents/Helpers/lns-service")
        XCTAssertEqual(launch.environment["LNS_SOCKET_PATH"], "/private/test/service.sock")
        XCTAssertEqual(launch.environment["LNS_HEADLESS"], "1")
        XCTAssertEqual(launch.environment["LNS_HOME"], "/isolated/data")
        XCTAssertEqual(launch.environment["KEEP_ME"], "value")
    }
}
