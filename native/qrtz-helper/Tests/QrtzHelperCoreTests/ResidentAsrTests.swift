import Foundation
import XCTest
@testable import QrtzHelperCore

/// A recogniser whose load takes `loadMs` and is shared the way NemotronTranscriber shares it (`SharedLoad`).
final class SlowLoadASR: StreamingTranscriber, @unchecked Sendable {
    let name = "slow"
    let loadsInstantly = false
    let lock = NSLock()
    var loads = 0, unloads = 0, received = 0
    let loadMs: UInt64
    private let load = SharedLoad<Int>()
    init(loadMs: UInt64 = 300) { self.loadMs = loadMs }
    func prepare() async throws {
        _ = try await load.get { [self] in
            let n = lock.withLock { loads += 1; return loads }
            try? await Task.sleep(nanoseconds: loadMs * 1_000_000)
            return n
        }
    }
    func append(_ s: [Float]) async { lock.withLock { received += s.count } }
    func transcript() async -> String { lock.withLock { received } > 0 ? "hello world" : "" }
    func finish() async -> String { await transcript() }
    func reset() async { lock.withLock { received = 0 } }
    func unload() async { lock.withLock { unloads += 1 }; load.clear() }
}

final class ResidentAsrTests: XCTestCase {
    func controller(_ asr: StreamingTranscriber, preload: Bool = true) -> (HelperController, Box<[HelperMessage]>) {
        let out = Box<[HelperMessage]>([])
        var deps = HelperDeps(permissions: FakePerms(), meetings: nil, audio: [], transcribers: [], helperVersion: "t", os: "linux",
                              features: [], dictationMic: FakeMic(), dictationTranscribers: [asr], insertion: FakeInsertion())
        deps.preloadDictationAsr = preload
        return (HelperController(deps: deps, send: { m in out.mutate { $0.append(m) } }, now: { 0 }), out)
    }

    func dictate(_ c: HelperController, _ id: String) async {
        await c.handle(.dictationStart(id: id))
        await c.dictation.onSamples([Float](repeating: 0.2, count: 3200))
        await c.handle(.dictationStop(id: id))
    }

    func stopped(_ out: Box<[HelperMessage]>, _ id: String) -> String? {
        out.value.compactMap { if case .dictationStopped(id, let t, _, _) = $0 { t } else { nil } }.first
    }

    /// ADR 0087: the helper loads echo's recogniser as it starts, and logs it resident.
    func testTheHelperPreloadsEchoAsrAtStart() async {
        let asr = SlowLoadASR(loadMs: 50)
        let (c, out) = controller(asr)
        await c.start()
        await (await c.startupPreload)?.value
        XCTAssertEqual(asr.loads, 1)
        XCTAssertTrue(out.value.contains { if case .log(_, let m) = $0 { m.hasPrefix("echo asr: slow resident (") } else { false } })
    }

    /// ADR 0087: a press while the startup load runs joins it: one load, and the dictation still gets its words.
    func testAPressDuringTheStartupPreloadJoinsTheSameLoad() async {
        let asr = SlowLoadASR(loadMs: 300)
        let (c, out) = controller(asr)
        await c.start()
        try? await Task.sleep(nanoseconds: 50_000_000)
        await dictate(c, "d1")
        await (await c.startupPreload)?.value
        XCTAssertEqual(asr.loads, 1)
        XCTAssertEqual(stopped(out, "d1"), "hello world")
    }

    /// ADR 0087: echo's instance stays loaded for the helper's life: dictations, a push-key prewarm, more dictations.
    func testEchoAsrIsNeverUnloaded() async {
        let asr = SlowLoadASR(loadMs: 20)
        let (c, out) = controller(asr)
        await c.start()
        await (await c.startupPreload)?.value
        for i in 0..<3 { await dictate(c, "d\(i)") }
        await c.handle(.dictationConfigure(warmMic: false, preRollMs: 0, streamAudio: false, pushKey: "fn"))
        try? await Task.sleep(nanoseconds: 50_000_000)
        await dictate(c, "d3")
        XCTAssertEqual(asr.loads, 1)
        XCTAssertEqual(asr.unloads, 0)
        XCTAssertEqual(stopped(out, "d3"), "hello world")
    }

    func testPreloadCanBeTurnedOff() async {
        let asr = SlowLoadASR(loadMs: 10)
        let (c, _) = controller(asr, preload: false)
        await c.start()
        let p = await c.startupPreload
        XCTAssertNil(p)
        try? await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(asr.loads, 0)
    }
}

final class SharedLoadTests: XCTestCase {
    final class Count: @unchecked Sendable { let lock = NSLock(); var n = 0; func bump() -> Int { lock.withLock { n += 1; return n } } }
    struct Boom: Error {}

    func testConcurrentCallersShareOneLoad() async throws {
        let l = SharedLoad<Int>(), c = Count()
        async let a = l.get { try? await Task.sleep(nanoseconds: 100_000_000); return c.bump() }
        async let b = l.get { c.bump() }
        let (x, y) = try await (a, b)
        XCTAssertEqual(x, y)
        XCTAssertEqual(c.n, 1)
        XCTAssertEqual(l.loaded, x)
        _ = try await l.get { c.bump() }  // loaded: returns at once
        XCTAssertEqual(c.n, 1)
    }

    func testAFailedLoadIsRetriedAndClearForgets() async throws {
        let l = SharedLoad<Int>(), c = Count()
        do { _ = try await l.get { _ = c.bump(); throw Boom() }; XCTFail() } catch {}
        XCTAssertNil(l.loaded)
        let v = try await l.get { c.bump() }
        XCTAssertEqual(v, 2)
        XCTAssertEqual(l.clear(), 2)
        XCTAssertNil(l.loaded)
        let w = try await l.get { c.bump() }
        XCTAssertEqual(w, 3)
    }
}
