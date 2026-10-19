import Foundation

// ADR 0221: the notch surface, the platform-free part (wire types, the arbiter, the shape and the geometry). It builds
// and tests on Linux; QrtzMacShared/NotchSurface.swift draws it. packages/ui/src/NotchSurface.tsx mirrors the shape and
// the geometry (NotchTests checks the constants against it).

/// Who holds the surface. Host-assigned on the wire (never module-supplied); the helper re-checks the order.
public enum NotchPriority: String, Sendable, CaseIterable {
    case system, ctrl, module
    /// Higher wins: system (echo's dictation) > ctrl run status > module requests.
    public var rank: Int {
        switch self {
        case .system: 3
        case .ctrl: 2
        case .module: 1
        }
    }
}

/// The fixed built-in icon set (module content names one; no bitmaps ever cross the wire).
public enum NotchIcon: String, Sendable, CaseIterable {
    case check, alert, info, mic, clock, download, bolt, calendar
    /// The SF Symbol the Mac draws.
    public var symbol: String {
        switch self {
        case .check: "checkmark.circle"
        case .alert: "exclamationmark.triangle"
        case .info: "info.circle"
        case .mic: "mic"
        case .clock: "clock"
        case .download: "arrow.down.circle"
        case .bolt: "bolt"
        case .calendar: "calendar"
        }
    }
}

public enum NotchTone: String, Sendable { case normal, error }

/// What the surface shows. No clickable actions in v1 (the window ignores the mouse; ADR 0221 future work).
public enum NotchContent: Equatable, Sendable {
    /// The waveform following the mic level (`busy`: the slow swell of processing).
    case level(busy: Bool)
    /// A line of status text (an error draws the alert icon).
    case status(text: String, tone: NotchTone)
    /// A label and a bar: `value` 0...1, nil = indeterminate.
    case progress(label: String, value: Double?)
    /// A built-in icon and a label.
    case icon(NotchIcon, label: String)

    public var accessibilityLabel: String {
        switch self {
        case .level(let busy): busy ? "processing" : "listening"
        case .status(let t, _): t
        case .progress(let l, let v): v.map { "\(l) \(Int(($0 * 100).rounded())) percent" } ?? l
        case .icon(_, let l): l
        }
    }

    public var json: JSONValue {
        switch self {
        case .level(let busy): return .object(["kind": .string("level"), "busy": .bool(busy)])
        case .status(let t, let tone): return .object(["kind": .string("status"), "text": .string(t), "tone": .string(tone.rawValue)])
        case .progress(let l, let v):
            var o: [String: JSONValue] = ["kind": .string("progress"), "label": .string(l)]
            if let v { o["value"] = .number(v) }
            return .object(o)
        case .icon(let i, let l): return .object(["kind": .string("icon"), "icon": .string(i.rawValue), "label": .string(l)])
        }
    }

    public init(json: JSONValue) throws {
        let f = try Fields(json)
        func text(_ k: String, _ max: Int) throws -> String {
            let t = try f.str(k)
            guard !t.isEmpty, t.utf16.count <= max else { throw ProtocolError("bad notch \(k)") }
            return t
        }
        switch try f.str("kind") {
        case "level":
            guard let b = f.o["busy"]?.bool else { throw ProtocolError("missing busy") }
            self = .level(busy: b)
        case "status": self = .status(text: try text("text", NotchLimits.statusMax), tone: try f.enumValue("tone", NotchTone.self))
        case "progress":
            let v = try f.optNum("value")
            if let v, !(v.isFinite && v >= 0 && v <= 1) { throw ProtocolError("bad progress value") }
            self = .progress(label: try text("label", NotchLimits.labelMax), value: v)
        case "icon": self = .icon(try f.enumValue("icon", NotchIcon.self), label: try text("label", NotchLimits.labelMax))
        case let k: throw ProtocolError("bad notch content \(k)")
        }
    }
}

/// The wire caps (the host sanitizes and caps module text to these before it sends; ADR 0221).
public enum NotchLimits {
    public static let labelMax = 60, statusMax = 120, ownerMax = 64, sourceMax = 40
}

/// One holder's request, as the helper keeps it.
public struct NotchEntry: Equatable, Sendable {
    public var owner: String
    public var priority: NotchPriority
    public var content: NotchContent
    /// The module's registered display name (module owners only; the host sets it from the manifest).
    public var source: String?
    public init(owner: String, priority: NotchPriority, content: NotchContent, source: String? = nil) {
        self.owner = owner; self.priority = priority; self.content = content; self.source = source
    }
}

/// The helper's side of the rules (the host enforces them first; this is the second check): one entry per priority,
/// the highest is drawn; a lower show while a higher holds is kept underneath, never drawn over it; only the holder of
/// a priority can update or dismiss it. Module entries need a source. Not thread-safe: the surface calls it on main.
public struct NotchArbiter: Sendable {
    public private(set) var entries: [NotchPriority: NotchEntry] = [:]
    public init() {}

    public var visible: NotchEntry? { entries.values.max { $0.priority.rank < $1.priority.rank } }

    /// false: refused (a module entry without a source).
    @discardableResult
    public mutating func show(_ e: NotchEntry) -> Bool {
        guard e.priority != .module || !(e.source ?? "").isEmpty else { return false }
        entries[e.priority] = e
        return true
    }

    @discardableResult
    public mutating func update(owner: String, content: NotchContent) -> Bool {
        guard let hit = entries.first(where: { $0.value.owner == owner }) else { return false }
        var n = hit.value; n.content = content
        entries[hit.key] = n
        return true
    }

    @discardableResult
    public mutating func dismiss(owner: String) -> Bool {
        guard let p = entries.first(where: { $0.value.owner == owner })?.key else { return false }
        entries[p] = nil
        return true
    }
}

/// One step of the shape's outline (y runs down from the screen's top edge).
public enum NotchPathOp: Equatable, Sendable {
    case move(Double, Double)
    case line(Double, Double)
    /// A quadratic curve: control point, then end point.
    case quad(Double, Double, Double, Double)
    case close
}

/// The notch shape, one function for the Mac (CAShapeLayer) and the web mock (SVG path, packages/ui NotchSurface.tsx
/// `notchPath`, the same math): a flat top flush with the screen's top edge, rounded bottom corners (`radius`) and
/// small concave ears (`ear`) where the sides meet the top, like the hardware notch. `width` includes the ears; the
/// body is `width - 2 * ear` wide. The op list always has the same 10 steps, so Core Animation can morph between any
/// two (rest <-> expanded).
public enum NotchShape {
    public static func clamp(width w: Double, height h: Double, radius r: Double, ear e: Double) -> (radius: Double, ear: Double) {
        let w = max(0, w), h = max(0, h)
        let ear = max(0, min(e, h / 2, w / 4))
        let radius = max(0, min(r, h - ear, (w - 2 * ear) / 2))
        return (radius, ear)
    }

    public static func path(width w0: Double, height h0: Double, radius r0: Double, ear e0: Double) -> [NotchPathOp] {
        let w = max(0, w0), h = max(0, h0)
        let (r, e) = clamp(width: w, height: h, radius: r0, ear: e0)
        return [
            .move(0, 0),
            .quad(e, 0, e, e),
            .line(e, h - r),
            .quad(e, h, e + r, h),
            .line(w - e - r, h),
            .quad(w - e, h, w - e, h - r),
            .line(w - e, e),
            .quad(w - e, 0, w, 0),
            .line(0, 0),
            .close,
        ]
    }
}

/// Sizes (pt). A notch Mac's rest state is exactly its notch; elsewhere a fake notch tab collapsing to zero height.
public enum NotchGeometry {
    public static let fakeNotchWidth = 180.0, fakeNotchHeight = 24.0
    /// The expanded shape: the notch plus `sideGrow` each side and `dropGrow` below it.
    public static let sideGrow = 70.0, dropGrow = 32.0
    /// The most a state can widen it, each side, past `sideGrow`.
    public static let widenMax = 90.0
    /// Spare panel width each side past the widest shape, so a spring's overshoot at `widenMax` is never clipped by
    /// the panel edge (bounce ≤ 0.35 overshoots ≤ ~7% of the up-to-168 pt side travel, ≈ 12 pt).
    public static let bounceRoom = 12.0
    public static let restRadius = 8.0, expandedRadius = 14.0, expandedEar = 8.0
    public static let padding = 14.0, gap = 8.0, iconSize = 14.0, fontSize = 13.158
    /// The panel never moves or resizes while shown: big enough for the widest state.
    public static func panelSize(notchWidth: Double, notchHeight: Double) -> (w: Double, h: Double) {
        (notchWidth + 2 * (sideGrow + widenMax + expandedEar + bounceRoom), notchHeight + dropGrow + 4)
    }

    /// The shape's width and height for a body widened by `extra` (0 = the normal expanded size), with its ears.
    public static func expanded(notchWidth: Double, notchHeight: Double, extra: Double = 0) -> (w: Double, h: Double) {
        let side = sideGrow + max(0, min(widenMax, extra))
        return (notchWidth + 2 * side + 2 * expandedEar, notchHeight + dropGrow)
    }

    /// How much wider than normal the body must be to fit `textWidth` (plus `leading`, an icon, on the content row).
    public static func extra(notchWidth: Double, contentWidth: Double) -> Double {
        let room = notchWidth + 2 * sideGrow - 2 * padding
        return max(0, min(widenMax, ((contentWidth - room) / 2).rounded(.up)))
    }
}

/// The notch surface's motion and corner values (ADR 0221). TEMPORARY, for tuning: the host sends
/// `talk.pillMotion` (echo's "dev opts" sliders) in `dictation.configure{pillMotion}`; the surface reads it each time
/// it builds an animation, so a change applies on the next show. Absent: these defaults (the snappy spring). Every
/// value is clamped to the same range as the zod schema (packages/core/src/config.ts); bounce stops at 0.35 so the
/// overshoot (≤ ~7% of the travel) stays inside the panel's spare width and 4 pt of bottom room.
public struct NotchMotion: Equatable, Sendable {
    public var expandDuration = 0.3, expandBounce = 0.15
    public var collapseDuration = 0.3, collapseBounce = 0.0
    public var fadeInDelay = 0.11, fadeInDuration = 0.09, fadeOutDuration = 0.06
    public var widenDuration = 0.2, widenBounce = 0.04
    public var radiusCollapsed = NotchGeometry.restRadius, radiusExpanded = NotchGeometry.expandedRadius
    public var shoulder = NotchGeometry.expandedEar
    public init() {}

    /// name, min, max (seconds, bounce 0..1, or pt).
    public static let ranges: [(String, Double, Double)] = [
        ("expandDuration", 0.05, 1.5), ("expandBounce", 0, 0.35),
        ("collapseDuration", 0.05, 1.5), ("collapseBounce", 0, 0.35),
        ("fadeInDelay", 0, 0.6), ("fadeInDuration", 0, 0.6), ("fadeOutDuration", 0, 0.1),
        ("widenDuration", 0.05, 1.5), ("widenBounce", 0, 0.35),
        ("radiusCollapsed", 0, 16), ("radiusExpanded", 0, 32), ("shoulder", 0, 16),
    ]

    /// Computed, not a stored static: key paths aren't Sendable, so a stored global fails Swift 6's concurrency check.
    private static var keys: [(String, WritableKeyPath<NotchMotion, Double>)] { [
        ("expandDuration", \.expandDuration), ("expandBounce", \.expandBounce),
        ("collapseDuration", \.collapseDuration), ("collapseBounce", \.collapseBounce),
        ("fadeInDelay", \.fadeInDelay), ("fadeInDuration", \.fadeInDuration), ("fadeOutDuration", \.fadeOutDuration),
        ("widenDuration", \.widenDuration), ("widenBounce", \.widenBounce),
        ("radiusCollapsed", \.radiusCollapsed), ("radiusExpanded", \.radiusExpanded), ("shoulder", \.shoulder),
    ] }

    /// From the wire object: unknown keys ignored, a missing or non-number key keeps its default, values clamped.
    public init(json o: [String: JSONValue]) {
        self.init()
        for (k, kp) in Self.keys {
            guard let v = o[k]?.number, v.isFinite, let r = Self.ranges.first(where: { $0.0 == k }) else { continue }
            self[keyPath: kp] = max(r.1, min(r.2, v))
        }
    }

    /// The content's fade when the surface hides. Secure input (a password field) hides it at once, whatever the
    /// motion settings; any other hide is capped at 0.1 s (also when built in code, past the wire clamp).
    public func hideContentFade(secure: Bool) -> Double { secure ? 0 : max(0, min(0.1, fadeOutDuration)) }

    public var json: JSONValue { .object(Dictionary(uniqueKeysWithValues: Self.keys.map { ($0.0, .number(self[keyPath: $0.1])) })) }
}

/// The screen facts the Mac reads (NSScreen), in Cocoa points, so the placement math is testable here.
public struct NotchScreen: Equatable, Sendable {
    public var frameX: Double, frameMaxY: Double, frameWidth: Double
    /// safeAreaInsets.top (0 without a notch).
    public var safeTop: Double
    /// auxiliaryTopLeftArea.maxX and auxiliaryTopRightArea.minX (nil without a notch).
    public var leftMaxX: Double?, rightMinX: Double?
    public init(frameX: Double, frameMaxY: Double, frameWidth: Double, safeTop: Double, leftMaxX: Double?, rightMinX: Double?) {
        self.frameX = frameX; self.frameMaxY = frameMaxY; self.frameWidth = frameWidth
        self.safeTop = safeTop; self.leftMaxX = leftMaxX; self.rightMinX = rightMinX
    }

    /// The hardware notch: x from the left area's maxX to the right area's minX, safeTop high. nil: no notch.
    public var notch: (minX: Double, width: Double, height: Double)? {
        guard safeTop > 0, let l = leftMaxX, let r = rightMinX, r > l else { return nil }
        return (l, r - l, safeTop)
    }

    /// The rest rect (notch, or the fake tab) and whether it is a hardware notch. Rest height 0 for the fake tab.
    public var rest: (minX: Double, width: Double, height: Double, real: Bool) {
        if let n = notch { return (n.minX, n.width, n.height, true) }
        let w = NotchGeometry.fakeNotchWidth
        return (frameX + (frameWidth - w) / 2, w, 0, false)
    }

    /// The height the shape's content row starts below (never under the physical notch).
    public var contentTop: Double { notch?.height ?? NotchGeometry.fakeNotchHeight }

    /// The panel's frame (origin bottom-left), centered on the rest rect, flush with the screen's top.
    public var panelFrame: (x: Double, y: Double, w: Double, h: Double) {
        let r = rest
        let s = NotchGeometry.panelSize(notchWidth: r.width, notchHeight: contentTop)
        return (r.minX + r.width / 2 - s.w / 2, frameMaxY - s.h, s.w, s.h)
    }
}
