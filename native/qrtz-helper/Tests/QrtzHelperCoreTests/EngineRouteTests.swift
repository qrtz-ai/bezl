import Foundation
import XCTest
@testable import QrtzEcho

final class StubBatch: BatchRecognizer, @unchecked Sendable {
    let name: String
    var failPrepare = false, failDecode = false
    let lock = NSLock()
    var decodes = 0, unloads = 0, prepared = false
    init(_ name: String) { self.name = name }
    func prepare() async throws {
        if failPrepare { throw NSError(domain: "stub", code: 1) }
        lock.withLock { prepared = true }
    }
    func decode(_ s: [Float]) async throws -> BatchResult {
        lock.withLock { decodes += 1 }
        if failDecode { throw NSError(domain: "stub", code: 2) }
        return BatchResult(text: name)
    }
    func unload() async { lock.withLock { unloads += 1; prepared = false } }
}

final class EngineRouteTests: XCTestCase {
    func samples(_ s: Double) -> [Float] { [Float](repeating: 0, count: Int((s * 16_000).rounded())) }

    func testDecideBoundaries() {
        func d(_ ready: Bool, _ s: Double, on: Bool = true) -> EngineRoute {
            EngineRoute.decide(enabled: on, secondaryReady: ready, seconds: s, primary: "p", secondary: "x")
        }
        XCTAssertEqual(d(true, 2.99), EngineRoute(engine: "p", fallback: .tooShort))
        XCTAssertEqual(d(true, 3), EngineRoute(engine: "x"))
        XCTAssertEqual(d(true, 35), EngineRoute(engine: "x"))
        XCTAssertEqual(d(true, 35.01), EngineRoute(engine: "p", fallback: .tooLong))
        for s in [2.99, 3, 35, 35.01] {
            XCTAssertEqual(d(false, s), EngineRoute(engine: "p", fallback: .notLoaded))
            XCTAssertEqual(d(true, s, on: false), EngineRoute(engine: "p"))
            XCTAssertEqual(d(false, s, on: false), EngineRoute(engine: "p"))
        }
    }

    func testRoutesOnlyWhenReadyAndInRange() async throws {
        let p = StubBatch("parakeet-tdt-v2"), x = StubBatch("phonon-2.5-en")
        let r = RoutedRecognizer(primary: p, secondary: x)
        try await r.prepare()
        var out = try await r.decode(samples(10))
        XCTAssertEqual(out.engine, "parakeet-tdt-v2"); XCTAssertNil(out.engineFallback)
        await r.setSecondary(enabled: true); await r.waitForSecondary()
        XCTAssertEqual(r.secondaryState, .ready)
        for (s, eng, fb) in [(2.99, "parakeet-tdt-v2", "too-short"), (3.0, "phonon-2.5-en", nil),
                             (35.0, "phonon-2.5-en", nil), (35.01, "parakeet-tdt-v2", "too-long")] as [(Double, String, String?)] {
            out = try await r.decode(samples(s))
            XCTAssertEqual(out.engine, eng, "\(s)"); XCTAssertEqual(out.engineFallback, fb, "\(s)"); XCTAssertEqual(out.text, eng)
        }
    }

    func testDecodeErrorFallsBack() async throws {
        let p = StubBatch("parakeet-tdt-v2"), x = StubBatch("phonon-2.5-en"); x.failDecode = true
        let r = RoutedRecognizer(primary: p, secondary: x)
        await r.setSecondary(enabled: true); await r.waitForSecondary()
        let out = try await r.decode(samples(5))
        XCTAssertEqual(out.engine, "parakeet-tdt-v2"); XCTAssertEqual(out.engineFallback, "error"); XCTAssertEqual(out.text, "parakeet-tdt-v2")
    }

    func testLoadErrorAndNotLoaded() async throws {
        let p = StubBatch("parakeet-tdt-v2"), x = StubBatch("phonon-2.5-en"); x.failPrepare = true
        let r = RoutedRecognizer(primary: p, secondary: x)
        await r.setSecondary(enabled: true); await r.waitForSecondary()
        XCTAssertEqual(r.secondaryState, .error)
        let out = try await r.decode(samples(5))
        XCTAssertEqual(out.engineFallback, "not-loaded"); XCTAssertEqual(x.decodes, 0)
    }

    func testToggleOffUnloadsSecondaryKeepsPrimary() async throws {
        let p = StubBatch("parakeet-tdt-v2"), x = StubBatch("phonon-2.5-en")
        let r = RoutedRecognizer(primary: p, secondary: x)
        try await r.prepare()
        await r.setSecondary(enabled: true); await r.waitForSecondary()
        await r.setSecondary(enabled: false)
        XCTAssertEqual(r.secondaryState, .off); XCTAssertEqual(x.unloads, 1); XCTAssertFalse(x.prepared)
        XCTAssertEqual(p.unloads, 0); XCTAssertTrue(p.prepared)
        let out = try await r.decode(samples(10))
        XCTAssertEqual(out.engine, "parakeet-tdt-v2"); XCTAssertNil(out.engineFallback); XCTAssertEqual(x.decodes, 0)
    }
}
