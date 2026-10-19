import Foundation

// ADR 0221: moved from QrtzEcho/Pill.swift so the shared notch surface (QrtzMacShared) can draw the waveform.

/// The wave, as in packages/ui/src/Waveform.tsx: three layered sines under a tapered envelope, the amplitude eased
/// toward the mic level each display frame (fast attack, slower release), a slow low swell while processing.
public enum PillWave {
    public enum Mode: Sendable { case live, idle }
    public static let liveFloor = 0.06
    public static let idleAmp = 0.14
    public static let attackMs = 60.0
    public static let releaseMs = 220.0
    public static func speed(_ m: Mode) -> Double { m == .live ? 7 : 2.2 }
    public struct Layer: Sendable { public let cycles, gain, opacity, rate, offset: Double }
    public static let layers = [
        Layer(cycles: 1.6, gain: 1, opacity: 1, rate: 1, offset: 0),
        Layer(cycles: 2.3, gain: 0.66, opacity: 0.5, rate: -1.3, offset: 1.9),
        Layer(cycles: 3.1, gain: 0.4, opacity: 0.28, rate: 1.7, offset: 4.1),
    ]
    public static let width = 160.0, height = 24.0, points = 48

    static func clamp01(_ x: Double) -> Double { x.isFinite ? min(1, max(0, x)) : 0 }

    public static func target(_ mode: Mode, _ level: Double) -> Double {
        mode == .idle ? idleAmp : liveFloor + (1 - liveFloor) * clamp01(level).squareRoot()
    }

    /// One frame of exponential smoothing toward `target` over `dtMs`; frame-rate independent.
    public static func step(_ cur: Double, _ target: Double, _ dtMs: Double) -> Double {
        let dt = max(0, min(dtMs, 250))
        let tau = target > cur ? attackMs : releaseMs
        return target + (cur - target) * exp(-dt / tau)
    }

    public static func envelope(_ u: Double) -> Double { pow(sin(Double.pi * clamp01(u)), 1.6) }

    /// One layer's points, y measured from the top like the SVG (the view flips).
    public static func layerPoints(amp: Double, phase: Double, cycles: Double, width: Double = width, height: Double = height) -> [(x: Double, y: Double)] {
        let mid = height / 2, a = clamp01(amp) * (mid - 1)
        return (0...points).map { i in
            let u = Double(i) / Double(points)
            return (u * width, mid + a * envelope(u) * sin(2 * Double.pi * cycles * u + phase))
        }
    }

    public static func frame(amp: Double, phase: Double) -> [[(x: Double, y: Double)]] {
        layers.map { layerPoints(amp: amp * $0.gain, phase: phase * $0.rate + $0.offset, cycles: $0.cycles) }
    }
}
