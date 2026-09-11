import Foundation

public enum SandboxProcess {
    public static func launch(_ launch: SandboxProcessLaunch) -> AsyncThrowingStream<SandboxLaunchEvent, Error> {
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

            func drain(_ handle: FileHandle, event: @escaping (Data) -> SandboxLaunchEvent) {
                readers.enter()
                DispatchQueue.global().async {
                    defer { handle.closeFile(); readers.leave() }
                    do {
                        while let bytes = try handle.read(upToCount: 4096), !bytes.isEmpty {
                            continuation.yield(event(bytes))
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
                drain(output.fileHandleForReading, event: SandboxLaunchEvent.output)
                drain(diagnostic.fileHandleForReading, event: SandboxLaunchEvent.diagnostic)
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
