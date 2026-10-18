#if os(macOS)
import FluidAudio
import Foundation
import QrtzMacShared
import QrtzNano
import QrtzShared

/// ADR 0079: FluidAudio Nemotron 3 diarization (`fast32`: 2.56 s of audio per call, about 2.9 s behind), one instance
/// per raw channel of a recording. Loads only from the verified local set (`LocalModels.diarizer`); never downloads.
/// Core ML calls are synchronous, so they run on the instance's own queue, off the cooperative pool.
public final class NemotronChannelDiarizer: ChannelDiarizer, @unchecked Sendable {
    public let name = "nemotron3-fast32"
    public init() {}
    private let config = Nemotron3Config.fast32
    private let queue = DispatchQueue(label: "qrtz.diarizer", qos: .userInitiated)
    private var diarizer: Nemotron3Diarizer?
    /// Its own lock (not the queue), so `abandon()` can flush while a model call still holds the queue (N121).
    private let trackerLock = NSLock()
    private var tracker = TurnTracker()

    private func onQueue<T>(_ f: @escaping () -> T) async -> T {
        await withCheckedContinuation { c in queue.async { c.resume(returning: f()) } }
    }

    public func prepare() async throws {
        if await onQueue({ self.diarizer != nil }) { return }
        let dir = try LocalModels.dir(LocalModels.diarizer)
        let models = try await Nemotron3Models.load(config: config, directory: dir)
        let d = Nemotron3Diarizer(config: config, models: models)
        await onQueue { if self.diarizer == nil { self.diarizer = d } }
    }

    public func reset() async { await onQueue { self.diarizer?.reset(); self.trackerLock.withLock { self.tracker.reset() } } }

    private func turns(_ rs: [Nemotron3ChunkResult]) -> [DiarizedTurn] {
        var out: [DiarizedTurn] = []
        trackerLock.withLock {
            for r in rs { out += tracker.feed(r.probabilities, frames: r.frameCount, speakers: r.numSpeakers) }
        }
        return out
    }

    public func append(_ s: [Float]) async -> [DiarizedTurn] {
        await onQueue {
            guard let d = self.diarizer else { return [] }
            d.appendAudio(s)
            do { return self.turns(try d.processBufferedAudio()) } catch { logErr("diarizer: \(error)"); return [] }
        }
    }

    public func finish() async -> [DiarizedTurn] {
        await onQueue {
            guard let d = self.diarizer else { return [] }
            var out: [DiarizedTurn] = []
            do { out = self.turns(try d.finishStream()) } catch { logErr("diarizer finish: \(error)") }
            return out + self.trackerLock.withLock { self.tracker.flush() }
        }
    }

    public func unload() async { await onQueue { self.diarizer = nil; self.trackerLock.withLock { self.tracker.reset() } } }

    public func abandon() -> [DiarizedTurn] { trackerLock.withLock { tracker.flush() } }
}
#endif
