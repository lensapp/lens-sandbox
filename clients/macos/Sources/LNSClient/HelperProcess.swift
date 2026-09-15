import Foundation
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

public enum HelperProcess {
    public static func launch(_ launch: HelperProcessLaunch) -> AsyncThrowingStream<HelperProcessEvent, Error> {
        AsyncThrowingStream { continuation in
            let process = Process()
            let output = Pipe()
            let diagnostic = Pipe()
            let readers = DispatchGroup()
            process.executableURL = launch.executable
            process.arguments = launch.arguments
            process.environment = launch.environment
            process.currentDirectoryURL = URL(fileURLWithPath: "/", isDirectory: true)
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = output
            process.standardError = diagnostic

            func drain(_ handle: FileHandle, event: @escaping (Data) -> HelperProcessEvent) {
                readers.enter()
                DispatchQueue.global().async {
                    defer { handle.closeFile(); readers.leave() }
                    do {
                        var buffer = [UInt8](repeating: 0, count: 4096)
                        while true {
                            let count = read(handle.fileDescriptor, &buffer, buffer.count)
                            if count < 0 {
                                if errno == EINTR { continue }
                                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
                            }
                            if count == 0 { break }
                            continuation.yield(event(Data(buffer.prefix(count))))
                        }
                    } catch { continuation.finish(throwing: error) }
                }
            }

            readers.enter()
            process.terminationHandler = { process in
                readers.notify(queue: .global()) {
                    continuation.yield(.exited(process.terminationStatus))
                    continuation.finish()
                }
            }
            continuation.onTermination = { _ in if process.isRunning { process.terminate() } }
            do {
                try process.run()
                drain(output.fileHandleForReading, event: HelperProcessEvent.output)
                drain(diagnostic.fileHandleForReading, event: HelperProcessEvent.diagnostic)
            } catch {
                output.fileHandleForReading.closeFile()
                diagnostic.fileHandleForReading.closeFile()
                continuation.finish(throwing: error)
            }
            output.fileHandleForWriting.closeFile()
            diagnostic.fileHandleForWriting.closeFile()
            readers.leave()
        }
    }
}
