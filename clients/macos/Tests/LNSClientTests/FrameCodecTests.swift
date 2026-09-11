import Foundation
import XCTest
@testable import LNSClient

final class FrameCodecTests: XCTestCase {
    private let pong = Data(#"{"type":"Pong"}"#.utf8)

    func testEncodesTheServicesActualFrameFormat() throws {
        let encoded = try FrameDecoder.encode(pong)
        XCTAssertEqual(Array(encoded.prefix(9)), [76, 78, 83, 50, 0, 0, 0, 16, 1])
        XCTAssertEqual(encoded.dropFirst(9), pong)
    }

    func testFragmentedFramesAreNotLostOrDuplicated() throws {
        let frame = Data([76, 78, 83, 50, 0, 0, 0, 16, 1]) + pong
        var decoder = FrameDecoder()
        var decoded: [Data] = []
        for byte in frame + frame {
            decoded += try decoder.append(Data([byte]))
        }
        XCTAssertEqual(decoded, [pong, pong])
        try decoder.finish()
    }

    func testMultipleFramesInOneRead() throws {
        let frame = Data([76, 78, 83, 50, 0, 0, 0, 16, 1]) + pong
        var decoder = FrameDecoder()
        XCTAssertEqual(try decoder.append(frame + frame), [pong, pong])
    }

    func testRejectsInvalidHeadersBeforeBufferingABody() {
        for header: [UInt8] in [
            [0, 78, 83, 50, 0, 0, 0, 16],
            [76, 78, 83, 50, 0, 0, 0, 0],
            [76, 78, 83, 50, 0, 16, 0, 1],
            [76, 78, 83, 50, 0, 0, 0, 1, 2]
        ] {
            var decoder = FrameDecoder()
            XCTAssertThrowsError(try decoder.append(Data(header)))
        }
    }

    func testEOFInTheMiddleOfAFrameIsAnError() throws {
        var decoder = FrameDecoder()
        _ = try decoder.append(Data([76, 78]))
        XCTAssertThrowsError(try decoder.finish())
    }

    func testOversizedOutgoingFramesAreRejected() {
        XCTAssertThrowsError(try FrameDecoder.encode(Data(repeating: 0, count: 1_048_576)))
    }
}
