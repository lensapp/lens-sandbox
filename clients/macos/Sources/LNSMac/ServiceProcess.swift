import Foundation
import LNSClient

enum ServiceProcess {
    static func start(_ launch: ServiceLaunch) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let process = Process()
            process.executableURL = launch.executable
            process.arguments = launch.arguments
            process.environment = launch.environment
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            process.terminationHandler = { process in
                if process.terminationStatus == 0 { continuation.resume() }
                else {
                    continuation.resume(throwing: ServiceError(message: "The bundled CLI could not start the service (exit \(process.terminationStatus)). Run its service start command in Terminal for details."))
                }
            }
            do { try process.run() }
            catch { continuation.resume(throwing: error) }
        }
    }
}
