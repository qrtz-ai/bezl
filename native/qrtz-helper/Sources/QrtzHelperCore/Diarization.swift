import Foundation

// ADR 0079: per-channel on-device diarization for nano (never for echo). Foundation-only so the turn logic is tested on
// Linux; the macOS executable supplies the model (FluidAudio Nemotron 3 diarization).

/// One speaker slot's stretch of speech on one channel, in ms from the start of that channel's stream.
public struct DiarizedTurn: Equatable, Sendable {
    public let speaker: Int
    public let startMs: Double
    public let endMs: Double
    public init(speaker: Int, startMs: Double, endMs: Double) { self.speaker = speaker; self.startMs = startMs; self.endMs = endMs }
}

/// A streaming diarizer for one raw channel (16 kHz mono). Speaker slots are 0...7, ordered by arrival, and only
/// meaningful within that channel.
public protocol ChannelDiarizer: AnyObject, Sendable {
    var name: String { get }
    /// Loads the model from local, already-downloaded files only (never fetches). Throws when they are missing.
    func prepare() async throws
    /// Starts a new stream (speaker slots and the clock restart at 0).
    func reset() async
    /// Feeds audio; returns the turns that ended in it.
    func append(_ samples16k: [Float]) async -> [DiarizedTurn]
    /// End of stream: every open turn.
    func finish() async -> [DiarizedTurn]
    /// Drops the model (the recording ended).
    func unload() async
}

/// Frame probabilities (10 ms frames, `speakers` per frame, row-major) → closed turns, across chunk boundaries.
/// A slot is active above `threshold`; a gap shorter than `bridgeFrames` doesn't end a turn; a turn shorter than
/// `minFrames` is dropped.
public struct TurnTracker: Sendable {
    public let threshold: Float
    public let minFrames: Int
    public let bridgeFrames: Int
    public let frameMs: Double
    private var frame = 0
    private var open: [Int: (start: Int, lastActive: Int)] = [:]

    public init(threshold: Float = 0.5, minFrames: Int = 20, bridgeFrames: Int = 30, frameMs: Double = 10) {
        self.threshold = threshold; self.minFrames = minFrames; self.bridgeFrames = bridgeFrames; self.frameMs = frameMs
    }

    public var framesSeen: Int { frame }

    public mutating func reset() { frame = 0; open.removeAll() }

    public mutating func feed(_ probabilities: [Float], frames: Int, speakers: Int) -> [DiarizedTurn] {
        var out: [DiarizedTurn] = []
        guard speakers > 0 else { return out }
        let n = min(frames, probabilities.count / speakers)
        for f in 0..<n {
            let t = frame + f
            for s in 0..<min(speakers, 8) {
                let active = probabilities[f * speakers + s] > threshold
                if active {
                    if let o = open[s] { open[s] = (o.start, t) } else { open[s] = (t, t) }
                } else if let o = open[s], t - o.lastActive > bridgeFrames {
                    if let turn = close(s, o) { out.append(turn) }
                    open[s] = nil
                }
            }
        }
        frame += n
        return out.sorted { $0.startMs < $1.startMs }
    }

    public mutating func flush() -> [DiarizedTurn] {
        let out = open.sorted { $0.key < $1.key }.compactMap { close($0.key, $0.value) }
        open.removeAll()
        return out.sorted { $0.startMs < $1.startMs }
    }

    private func close(_ s: Int, _ o: (start: Int, lastActive: Int)) -> DiarizedTurn? {
        let end = o.lastActive + 1
        guard end - o.start >= minFrames else { return nil }
        return DiarizedTurn(speaker: s, startMs: Double(o.start) * frameMs, endMs: Double(end) * frameMs)
    }
}
