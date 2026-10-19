// qrtz snap's frame math (ADR 0159): a line-for-line port of Spectacle's window position calculations
// (github.com/eczarny/spectacle, Spectacle/Resources/Window Position Calculations/*.js and the movers' math).
//
// Spectacle is distributed under the MIT License:
// Copyright (c) 2017 Eric Czarny <eczarny@gmail.com>
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
// rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
// permit persons to whom the Software is furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all copies or substantial portions of
// the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
// WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
// COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
// OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//
// Foundation-only, so every action and cycle is unit tested on Linux. Rects are in Cocoa screen space (origin at the
// bottom left of the primary display, y grows up), exactly like Spectacle's calculations; `SnapRect.flipped` converts
// to and from the Accessibility API's top-left space.
import Foundation

public struct SnapRect: Equatable, Codable, Sendable, CustomStringConvertible {
    public var x: Double, y: Double, width: Double, height: Double
    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x; self.y = y; self.width = width; self.height = height
    }
    public var minX: Double { x }
    public var minY: Double { y }
    public var maxX: Double { x + width }
    public var maxY: Double { y + height }
    public var midX: Double { x + width / 2 }
    public var midY: Double { y + height / 2 }
    public var area: Double { width * height }
    public var isEmpty: Bool { width <= 0 || height <= 0 }
    public var description: String { "(\(x), \(y), \(width), \(height))" }

    /// CGRectContainsRect.
    public func contains(_ r: SnapRect) -> Bool {
        !isEmpty && !r.isEmpty && r.minX >= minX && r.maxX <= maxX && r.minY >= minY && r.maxY <= maxY
    }

    /// CGRectIntersection (nil for CGRectNull).
    public func intersection(_ r: SnapRect) -> SnapRect? {
        let x0 = max(minX, r.minX), x1 = min(maxX, r.maxX), y0 = max(minY, r.minY), y1 = min(maxY, r.maxY)
        guard x1 > x0, y1 > y0 else { return nil }
        return SnapRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    /// Spectacle's `normalizeCoordinatesOfRect`: flips between Cocoa (bottom-left) and AX (top-left) space against
    /// the primary display's height. Its own inverse.
    public func flipped(primaryHeight: Double) -> SnapRect {
        SnapRect(x: x, y: primaryHeight - maxY, width: width, height: height)
    }
}

/// Every Spectacle action (SpectacleWindowAction), named like its shortcut (`MoveToLeftHalf` → `leftHalf`).
public enum SnapAction: String, CaseIterable, Codable, Sendable {
    case center, fullscreen
    case leftHalf, rightHalf, topHalf, bottomHalf
    case upperLeft, lowerLeft, upperRight, lowerRight
    case nextDisplay, previousDisplay
    case nextThird, previousThird
    case makeLarger, makeSmaller
    case undo, redo

    public var movesToDisplay: Bool { self == .nextDisplay || self == .previousDisplay }
    public var isHistory: Bool { self == .undo || self == .redo }
    /// The lowercase label the module view and the status menu show.
    public var label: String {
        switch self {
        case .center: return "center"
        case .fullscreen: return "fullscreen"
        case .leftHalf: return "left half"
        case .rightHalf: return "right half"
        case .topHalf: return "top half"
        case .bottomHalf: return "bottom half"
        case .upperLeft: return "upper left"
        case .lowerLeft: return "lower left"
        case .upperRight: return "upper right"
        case .lowerRight: return "lower right"
        case .nextDisplay: return "next display"
        case .previousDisplay: return "previous display"
        case .nextThird: return "next third"
        case .previousThird: return "previous third"
        case .makeLarger: return "make larger"
        case .makeSmaller: return "make smaller"
        case .undo: return "undo last move"
        case .redo: return "redo last move"
        }
    }
}

public enum SnapGeometry {
    /// SpectacleCalculationHelpers.rectCenteredWithinRect.
    public static func centered(_ r1: SnapRect, _ r2: SnapRect) -> Bool {
        r1.contains(r2) && abs(r2.midX - r1.midX) <= 1.0 && abs(r2.midY - r1.midY) <= 1.0
    }

    /// SpectacleCalculationHelpers.rectFitsWithinRect.
    public static func fits(_ r1: SnapRect, _ r2: SnapRect) -> Bool {
        r1.width <= r2.width && r1.height <= r2.height
    }

    /// JavaScript's Math.round (halves go up).
    static func jsRound(_ v: Double) -> Double { (v + 0.5).rounded(.down) }

    /// The frame `action` gives `window` (nil for undo/redo, which replay the history instead).
    /// `source` and `destination` are the visible frames of the window's display and the target display.
    public static func calculate(_ action: SnapAction, window w: SnapRect, source: SnapRect, destination v: SnapRect) -> SnapRect? {
        switch action {
        case .center: return center(w, in: v)
        case .fullscreen: return v
        case .leftHalf:
            var half = v
            half.width = (half.width / 2).rounded(.down)
            return cycle(w, v, base: half, widths: true, anchorEnd: false)
        case .rightHalf:
            var half = v
            half.width = (half.width / 2).rounded(.down)
            half.x += half.width
            return cycle(w, v, base: half, widths: true, anchorEnd: true)
        case .topHalf:
            var half = v
            half.height = (half.height / 2).rounded(.down)
            half.y += half.height + v.height.truncatingRemainder(dividingBy: 2)
            return cycle(w, v, base: half, widths: false, anchorEnd: true)
        case .bottomHalf:
            var half = v
            half.height = (half.height / 2).rounded(.down)
            return cycle(w, v, base: half, widths: false, anchorEnd: false)
        case .upperLeft, .lowerLeft, .upperRight, .lowerRight:
            var q = v
            q.width = (v.width / 2).rounded(.down)
            q.height = (v.height / 2).rounded(.down)
            let right = action == .upperRight || action == .lowerRight
            if right { q.x += q.width }
            if action == .upperLeft || action == .upperRight {
                q.y = v.y + (v.height / 2).rounded(.down) + v.height.truncatingRemainder(dividingBy: 2)
            }
            return cycle(w, v, base: q, widths: true, anchorEnd: right)
        case .nextDisplay, .previousDisplay:
            return fits(w, v) ? center(w, in: v) : v
        case .nextThird: return third(w, v, step: 1)
        case .previousThird: return third(w, v, step: -1)
        case .makeLarger: return resize(w, v, by: 30)
        case .makeSmaller: return resize(w, v, by: -30)
        case .undo, .redo: return nil
        }
    }

    /// SpectacleCenterWindowCalculation.
    public static func center(_ w: SnapRect, in v: SnapRect) -> SnapRect {
        var r = w
        r.x = jsRound((v.width - w.width) / 2.0) + v.x
        r.y = jsRound((v.height - w.height) / 2.0) + v.y
        return r
    }

    /// The halves' and quarters' cycle: pressing the same action again goes 1/2 → 2/3 → 1/3 → 1/2 of the display
    /// (widths for left/right and the corners, heights for top/bottom), while the window stays on that edge.
    static func cycle(_ w: SnapRect, _ v: SnapRect, base: SnapRect, widths: Bool, anchorEnd: Bool) -> SnapRect {
        // Spectacle checks the other axis' midpoint first: only a window already on this row/column cycles.
        let aligned = widths ? abs(w.midY - base.midY) <= 1.0 : abs(w.midX - base.midX) <= 1.0
        guard aligned else { return base }
        // Math.floor(len * 2 / 3.0) and Math.floor(len / 3.0): multiply first, like the JS.
        func sized(_ num: Double) -> SnapRect {
            var r = base
            if widths {
                r.width = (v.width * num / 3.0).rounded(.down)
                if anchorEnd { r.x = v.x + v.width - r.width }
            } else {
                r.height = (v.height * num / 3.0).rounded(.down)
                if anchorEnd { r.y = v.y + v.height - r.height }
            }
            return r
        }
        let two = sized(2), one = sized(1)
        if centered(base, w) { return two }
        if centered(two, w) { return one }
        return base
    }

    /// SpectacleNextOrPreviousThirds: three columns left to right, then three rows top to bottom; a window centered
    /// in one moves to the next (or previous), wrapping; any other window goes to the first column.
    public static func thirds(_ v: SnapRect) -> [SnapRect] {
        var out: [SnapRect] = []
        let tw = (v.width / 3.0).rounded(.down), th = (v.height / 3.0).rounded(.down)
        for i in 0..<3 {
            var t = v
            t.x = v.x + tw * Double(i)
            t.width = tw
            out.append(t)
        }
        for i in 0..<3 {
            var t = v
            t.y = v.y + v.height - th * Double(i + 1)
            t.height = th
            out.append(t)
        }
        return out
    }

    static func third(_ w: SnapRect, _ v: SnapRect, step: Int) -> SnapRect {
        let ts = thirds(v)
        for (i, t) in ts.enumerated() where centered(t, w) {
            return ts[(i + step + ts.count) % ts.count]
        }
        return ts[0]
    }

    // MARK: SpectacleWindowSizeAdjuster (make larger / smaller by 30 points)

    static func againstEdge(_ gap: Double) -> Bool { abs(gap) <= 5.0 }
    static func againstLeft(_ w: SnapRect, _ v: SnapRect) -> Bool { againstEdge(w.x - v.x) }
    static func againstRight(_ w: SnapRect, _ v: SnapRect) -> Bool { againstEdge(w.maxX - v.maxX) }
    static func againstTop(_ w: SnapRect, _ v: SnapRect) -> Bool { againstEdge(w.maxY - v.maxY) }
    static func againstBottom(_ w: SnapRect, _ v: SnapRect) -> Bool { againstEdge(w.y - v.y) }

    public static func resize(_ w: SnapRect, _ v: SnapRect, by offset: Double) -> SnapRect {
        let half = (offset / 2.0).rounded(.down)
        var r = w
        r.width += offset
        r.x -= half
        // Left and right edges.
        if againstRight(w, v) {
            r.x = v.maxX - r.width
            if againstLeft(w, v) { r.width = v.width }
        }
        if againstLeft(w, v) { r.x = v.x }
        if r.width >= v.width { r.width = v.width }
        r.height += offset
        r.y -= half
        // Top and bottom edges.
        if againstTop(w, v) {
            r.y = v.maxY - r.height
            if againstBottom(w, v) { r.height = v.height }
        }
        if againstBottom(w, v) { r.y = v.y }
        if r.height >= v.height {
            r.height = v.height
            r.y = w.y
        }
        if againstLeft(w, v) && againstRight(w, v) && againstTop(w, v) && againstBottom(w, v) && offset < 0 {
            r.width = w.width + offset
            r.x = w.x - half
            r.height = w.height + offset
            r.y = w.y - half
        }
        if r.width <= (v.width / 4.0).rounded(.down) || r.height <= (v.height / 4.0).rounded(.down) {
            return w
        }
        return r
    }

    // MARK: the movers' math (SpectacleQuantizedWindowMover, SpectacleBestEffortWindowMover)

    /// SpectacleBestEffortWindowMover: a window that came out wider or taller than asked (an app with a minimum
    /// size) is pushed back inside the visible frame, in AX space against the primary display.
    public static func bestEffort(moved: SnapRect, visible: SnapRect, primaryHeight: Double) -> SnapRect {
        var m = moved
        if m.x < visible.x {
            m.x = visible.x
        } else if m.maxX > visible.maxX {
            m.x = visible.maxX - m.width
        }
        // The vertical check runs in Cocoa space, like Spectacle's normalize / check / normalize.
        var c = m.flipped(primaryHeight: primaryHeight)
        if c.y < visible.y {
            c.y = visible.y
        } else if c.maxY > visible.maxY {
            c.y = visible.maxY - c.height
        }
        m = c.flipped(primaryHeight: primaryHeight)
        return m
    }

    /// SpectacleQuantizedWindowMover's last step: a window that stayed bigger than asked (a terminal snapping to
    /// its character grid) is centered on the requested frame.
    public static func quantizedOrigin(requested: SnapRect, adjusted: SnapRect, moved: SnapRect) -> SnapRect {
        var a = adjusted
        a.x += ((requested.width - moved.width) / 2.0).rounded(.down)
        a.y += ((requested.height - moved.height) / 2.0).rounded(.down)
        return a
    }
}
