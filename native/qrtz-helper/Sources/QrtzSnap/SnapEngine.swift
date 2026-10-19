// qrtz snap's window manager (ADR 0159): SpectacleWindowPositionManager and its mover chain (Standard → Quantized →
// BestEffort), ported from Spectacle (MIT, Copyright (c) 2017 Eric Czarny; notice in SnapGeometry.swift). The macOS
// Accessibility side is behind `SnapWindowSystem`, so the whole flow is tested on Linux with a fake.
import Foundation

public struct SnapFrontmost<Element> {
    public var bundleId: String
    public var window: Element
    public init(bundleId: String, window: Element) { self.bundleId = bundleId; self.window = window }
}

public protocol SnapWindowSystem: AnyObject {
    associatedtype Element
    /// The focused window of the frontmost app, or nil (no app, no window, or no Accessibility grant).
    func frontmost() -> SnapFrontmost<Element>?
    /// The element's frame in AX space (top-left origin), or nil if it can't be read.
    func rect(of e: Element) -> SnapRect?
    /// Sheets and system dialogs are never moved (Spectacle beeps).
    func isSheetOrDialog(_ e: Element) -> Bool
    /// Size, then position, then size again, like SpectacleAccessibilityElement's setRectOfElement.
    func setRect(_ r: SnapRect, of e: Element)
    var screens: [SnapScreen] { get }
    var mainScreen: SnapScreen? { get }
    /// The primary display's height (NSScreen.screens[0]), for flipping between AX and Cocoa space.
    var primaryHeight: Double { get }
}

public enum SnapOutcome: Equatable {
    case moved(SnapRect)
    /// Spectacle's failure feedback (a beep): nothing moved.
    case failed(String)
}

public final class SnapEngine<System: SnapWindowSystem> {
    let system: System
    var histories: [String: SnapHistory<System.Element>] = [:]
    /// Apps snap never moves (Spectacle's BlacklistedApplications default) plus the user's disabled apps.
    public var disabledApps: Set<String> = []
    public static var alwaysDisabled: Set<String> { ["com.adobe.Photoshop", "com.divisiblebyzero.Spectacle", "com.valvesoftware.steam"] }

    public init(system: System) { self.system = system }

    public func isDisabled(_ bundleId: String) -> Bool {
        Self.alwaysDisabled.contains(bundleId) || disabledApps.contains(bundleId)
    }

    public func perform(_ action: SnapAction) -> SnapOutcome {
        guard let front = system.frontmost() else { return .failed("no focused window") }
        if isDisabled(front.bundleId) { return .failed("snap is off for \(front.bundleId)") }
        let history = histories[front.bundleId] ?? SnapHistory()
        histories[front.bundleId] = history
        switch action {
        case .undo: return replay(history.previous())
        case .redo: return replay(history.next())
        default: return move(front.window, action: action, history: history)
        }
    }

    func move(_ e: System.Element, action: SnapAction, history: SnapHistory<System.Element>) -> SnapOutcome {
        let ordered = SnapScreens.consistentOrder(system.screens)
        let ph = system.primaryHeight
        guard !system.isSheetOrDialog(e) else { return .failed("sheets and dialogs don't move") }
        guard let axRect = system.rect(of: e) else { return .failed("the window's frame can't be read") }
        guard let source = SnapScreens.containing(axRect, screens: ordered, main: system.mainScreen, primaryHeight: ph)
        else { return .failed("no display") }
        var destination = source
        if action.movesToDisplay {
            guard let d = SnapScreens.neighbour(of: source, next: action == .nextDisplay, screens: ordered) else {
                return .failed("only one display")
            }
            destination = d
        }
        if history.isEmpty { history.add(element: e, rect: axRect) }
        let before = axRect.flipped(primaryHeight: ph)
        guard let after = SnapGeometry.calculate(action, window: before, source: source.visibleFrame,
                                                 destination: destination.visibleFrame)
        else { return .failed("no calculation") }
        if after == before { return .failed("already there") }
        let target = after.flipped(primaryHeight: ph)
        history.add(element: e, rect: target)
        moveChain(e, to: target, visible: destination.visibleFrame)
        return .moved(target)
    }

    func replay(_ item: SnapHistory<System.Element>.Item?) -> SnapOutcome {
        guard let item else { return .failed("nothing to undo or redo") }
        let ph = system.primaryHeight
        let ordered = SnapScreens.consistentOrder(system.screens)
        guard let screen = SnapScreens.containing(item.rect, screens: ordered, main: system.mainScreen, primaryHeight: ph)
        else { return .failed("no display") }
        moveChain(item.element, to: item.rect, visible: screen.visibleFrame)
        return .moved(item.rect)
    }

    /// Standard (set the frame) → Quantized (shrink by 2pt until it fits, then center) → BestEffort (back inside).
    func moveChain(_ e: System.Element, to target: SnapRect, visible: SnapRect) {
        guard system.rect(of: e) != nil else { return }
        system.setRect(target, of: e)
        // Quantized.
        if var moved = system.rect(of: e), moved != target {
            var adjusted = target
            while moved.width > target.width || moved.height > target.height {
                if moved.width > target.width { adjusted.width -= 2 }
                if moved.height > target.height { adjusted.height -= 2 }
                if adjusted.width < target.width * 0.85 || adjusted.height < target.height * 0.85 { break }
                system.setRect(adjusted, of: e)
                guard let m = system.rect(of: e) else { break }
                moved = m
            }
            system.setRect(SnapGeometry.quantizedOrigin(requested: target, adjusted: adjusted, moved: moved), of: e)
        }
        // Best effort.
        if let moved = system.rect(of: e) {
            let fixed = SnapGeometry.bestEffort(moved: moved, visible: visible, primaryHeight: system.primaryHeight)
            if fixed != moved { system.setRect(fixed, of: e) }
        }
    }
}
