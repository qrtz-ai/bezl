import XCTest
@testable import QrtzHelperCore
@testable import QrtzEcho
@testable import QrtzNano
@testable import QrtzShared

/// N29: per-source silent-capture detection (capture.silent / capture.audible).
final class SilenceTests: XCTestCase {
    func testTrackerReportsOncePerStretchPerSourceAndClearsOnSound() {
        var t = SilenceTracker(config: SilenceConfig(floorRms: 0.001, afterMs: 1000))
        XCTAssertNil(t.update(.system, rms: 0, atMs: 0))
        XCTAssertNil(t.update(.mic, rms: 0.2, atMs: 0))
        XCTAssertNil(t.update(.system, rms: 0.0005, atMs: 999), "under the floor, not long enough yet")
        XCTAssertEqual(t.update(.system, rms: 0, atMs: 1000), .captureSilent(source: .system, sinceMs: 0))
        XCTAssertNil(t.update(.system, rms: 0, atMs: 5000), "once per stretch")
        XCTAssertTrue(t.isSilent(.system)); XCTAssertFalse(t.isSilent(.mic))
        XCTAssertEqual(t.update(.system, rms: 0.01, atMs: 5100), .captureAudible(source: .system))
        XCTAssertNil(t.update(.system, rms: 0.01, atMs: 5200))
        // A new stretch starts over; a blip above the floor resets it.
        XCTAssertNil(t.update(.mic, rms: 0, atMs: 6000))
        XCTAssertNil(t.update(.mic, rms: 0.5, atMs: 6500))
        XCTAssertNil(t.update(.mic, rms: 0, atMs: 6600))
        XCTAssertNil(t.update(.mic, rms: 0, atMs: 7500))
        XCTAssertEqual(t.update(.mic, rms: 0, atMs: 7600), .captureSilent(source: .mic, sinceMs: 6600))
        // A pause breaks the stretch but keeps what was reported.
        XCTAssertNil(t.update(.system, rms: 0, atMs: 8000))
        t.pause()
        XCTAssertNil(t.update(.system, rms: 0, atMs: 8900))
        XCTAssertNil(t.update(.system, rms: 0, atMs: 9800), "the stretch restarted at 8900")
        XCTAssertEqual(t.update(.system, rms: 0, atMs: 9900), .captureSilent(source: .system, sinceMs: 8900))
        XCTAssertTrue(t.isSilent(.mic))
    }

    func testDefaultIsAMinuteUnderMinus70dBFS() {
        XCTAssertEqual(SilenceConfig.default, SilenceConfig(floorRms: 0.0003, afterMs: 60_000))
    }

    func testControllerReportsTheSilentSourceFromRawChannels() async throws {
        let out = Box<[HelperMessage]>([]), clock = Box<Double>(1000), audio = FakeAudio("sck")
        let c = HelperController(
            deps: HelperDeps(permissions: FakePerms(), meetings: nil, audio: [audio], transcribers: [FakeASR()], helperVersion: "t", os: "linux", features: [],
                             silence: SilenceConfig(floorRms: 0.001, afterMs: 2000)),
            send: { m in out.mutate { $0.append(m) } }, now: { clock.value })
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("qrtz-sil-\(UUID()).wav").path
        await c.handle(.recordingStart(audioPath: path))
        let zeros = [Float](repeating: 0, count: 1600), loud = [Float](repeating: 0.2, count: 1600)
        func feed(_ b: CaptureBlock, at ms: Double) async throws {
            clock.mutate { $0 = ms }
            audio.cb?(b)
            try await Task.sleep(nanoseconds: 30_000_000)
        }
        func until(_ cond: () -> Bool) async throws {
            let end = Date().addingTimeInterval(5)
            while !cond() && Date() < end { try await Task.sleep(nanoseconds: 2_000_000) }
            XCTAssertTrue(cond(), "condition not reached in time")
        }
        let silent = { (s: CaptureSource) in out.value.contains { if case .captureSilent(s, _) = $0 { true } else { false } } }
        // System channel dead, mic fine: only the system source is reported, with the recording-relative start.
        try await feed(CaptureBlock(system: zeros, mic: loud), at: 1500)
        try await feed(CaptureBlock(system: zeros, mic: loud), at: 4000)
        try await until { silent(.system) }
        XCTAssertTrue(out.value.contains(.captureSilent(source: .system, sinceMs: 500)))
        XCTAssertFalse(silent(.mic))
        try await feed(CaptureBlock(system: loud, mic: loud), at: 4100)
        try await until { out.value.contains(.captureAudible(source: .system)) }
        // A source without a mic channel never reports the mic.
        try await feed(CaptureBlock(system: loud, mic: nil), at: 9000)
        XCTAssertFalse(silent(.mic))
        await c.handle(.recordingStop())
        try? FileManager.default.removeItem(atPath: path)
    }
}
