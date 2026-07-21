import Foundation
import Testing
@testable import BandicotFoundationModelsBridgeCore

@Test func frameRoundTrip() throws {
    let pipe = Pipe()
    let expected = BridgeFrame.snapshot(text: "hello")
    try writeFrame(expected, to: pipe.fileHandleForWriting)
    pipe.fileHandleForWriting.closeFile()
    let actual = try readFrame(BridgeFrame.self, from: pipe.fileHandleForReading)
    #expect(actual == expected)
}

@Test func errorsAreBounded() {
    #expect(bounded(String(repeating: "x", count: maximumErrorCharacters + 50)).count == maximumErrorCharacters)
}
