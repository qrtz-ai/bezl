// qrtz snap's display handling (ADR 0159), ported from SpectacleScreenDetector (MIT, Copyright (c) 2017 Eric Czarny;
// the full notice is in SnapGeometry.swift). Foundation-only: a display is its frame and visible frame in Cocoa space.
import Foundation

public struct SnapScreen: Equatable, Sendable {
    public var frame: SnapRect
    public var visibleFrame: SnapRect
    public init(frame: SnapRect, visibleFrame: SnapRect) { self.frame = frame; self.visibleFrame = visibleFrame }
    var atOrigin: Bool { frame.x == 0 && frame.y == 0 }
}

public enum SnapScreens {
    /// `screensInConsistentOrder`: the primary display (origin 0,0) first, then by x, then y, both descending, as
    /// Spectacle's two stable sorts leave them. Next/previous display walks this order and wraps.
    public static func consistentOrder(_ screens: [SnapScreen]) -> [SnapScreen] {
        func sorted(_ s: [SnapScreen], _ key: (SnapScreen) -> Double) -> [SnapScreen] {
            s.enumerated().sorted { a, b in
                if a.element.atOrigin != b.element.atOrigin { return a.element.atOrigin }
                if a.element.atOrigin && b.element.atOrigin { return a.offset < b.offset }
                // (NSComparisonResult)(two - one): truncated toward zero, so sub-point gaps compare equal.
                let d = Int(key(b.element) - key(a.element))
                return d == 0 ? a.offset < b.offset : d < 0
            }.map(\.element)
        }
        return sorted(sorted(screens, { $0.frame.y }), { $0.frame.x })
    }

    /// `screenContainingRect`: the first display that holds the whole window, else the one with the largest share
    /// of it, else `main`. `window` is in AX space.
    public static func containing(_ window: SnapRect, screens: [SnapScreen], main: SnapScreen?, primaryHeight: Double) -> SnapScreen? {
        var best = 0.0
        var result = main
        let w = window.flipped(primaryHeight: primaryHeight)
        for s in screens {
            if s.frame.contains(w) { return s }
            let share = (s.frame.intersection(w)?.area ?? 0) / max(w.area, 1)
            if share > best { best = share; result = s }
        }
        return result
    }

    /// `nextOrPreviousScreenToFrameOfScreen`: nil with one display (Spectacle beeps).
    public static func neighbour(of source: SnapScreen, next: Bool, screens: [SnapScreen]) -> SnapScreen? {
        guard screens.count > 1, let i = screens.firstIndex(where: { $0.frame == source.frame }) else { return nil }
        let j = (i + (next ? 1 : -1) + screens.count) % screens.count
        return screens[j]
    }
}

/// SpectacleHistory: per app, a doubly linked list of the frames a window had (the original first), at most 50.
/// Undo steps back and redo forward; a new move after an undo drops the redo chain (Spectacle leaves the new item
/// with no next).
public final class SnapHistory<Element> {
    public struct Item {
        public let element: Element
        public let rect: SnapRect
    }
    private var items: [Item] = []
    private var current = -1
    public static var limit: Int { 50 }
    public init() {}

    public var isEmpty: Bool { items.isEmpty }
    public var count: Int { items.count }

    public func add(_ item: Item) {
        items.removeSubrange((current + 1)...)
        items.append(item)
        current = items.count - 1
        if items.count > Self.limit {
            items.removeFirst()
            current -= 1
        }
    }

    public func add(element: Element, rect: SnapRect) { add(Item(element: element, rect: rect)) }

    public func previous() -> Item? {
        guard current > 0 else { return nil }
        current -= 1
        return items[current]
    }

    public func next() -> Item? {
        guard current + 1 < items.count else { return nil }
        current += 1
        return items[current]
    }
}
