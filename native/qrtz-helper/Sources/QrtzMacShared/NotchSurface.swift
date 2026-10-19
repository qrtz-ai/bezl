#if os(macOS)
import AppKit
import Carbon.HIToolbox
import QrtzShared
import QuartzCore

// ADR 0221: the notch surface, qrtz's one top-of-screen status surface (echo's dictation is one client; ctrl run
// status and module requests reach it only through the host). A black notch-shaped layer flush with the top edge
// grows down and out of the MacBook notch (or a fake notch tab elsewhere), draws its content below the notch, and
// retracts back into it. Main thread only. Nothing here is ever awaited by capture, release or insertion.

/// Borderless, non-activating, never key or main; its frame may sit over the menu bar (AppKit would push it down).
final class NotchPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

public final class NotchSurface: @unchecked Sendable {
    /// echo's dictation: the helper's own owner, the only `.system` one (the helper drops host `.system` shows).
    public static let dictationOwner = "echo.dictation"

    private let panel: NotchPanel
    private let stage: NotchStageView
    private var arbiter = NotchArbiter()
    private var screenObserver: NSObjectProtocol?
    private var secureTimer: Timer?
    /// What is drawn now (nil: hidden or retracting).
    private var drawn: NotchEntry?
    private var hideGeneration = 0

    /// `fontName`: the content's font (echo registers Geist Pixel Square); the system font when nil or unknown.
    public init(fontName: String?) {
        let p = NotchPanel(contentRect: NSRect(x: 0, y: 0, width: 400, height: 80),
                           styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        p.isFloatingPanel = true
        // Above the menu bar and its status items (mainMenu 24, statusBar 25), below pop-up menus and the screen
        // saver/shielding levels, so system alerts and the login window still cover it (the NotchNook-style level).
        p.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        p.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        p.ignoresMouseEvents = true  // no clicks in v1: a top-of-screen click target is a clickjacking risk
        p.hidesOnDeactivate = false
        p.becomesKeyOnlyIfNeeded = true
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = false
        p.isReleasedWhenClosed = false
        p.animationBehavior = .none
        let v = NotchStageView(frame: NSRect(x: 0, y: 0, width: 400, height: 80), fontName: fontName)
        p.contentView = v
        panel = p; stage = v
        screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.screensChanged() }
    }

    deinit {
        if let o = screenObserver { NotificationCenter.default.removeObserver(o) }
        secureTimer?.invalidate()
    }

    /// qrtz's theme (ADR 0211) on the panel: the shape stays black either way; it only reaches the content's accent.
    public var appearance: NSAppearance? {
        get { panel.appearance }
        set { panel.appearance = newValue; stage.recolor() }
    }

    public func show(_ e: NotchEntry) {
        guard arbiter.show(e) else { return }
        render()
    }

    public func update(owner: String, content: NotchContent) {
        guard arbiter.update(owner: owner, content: content) else { return }
        render()
    }

    public func dismiss(owner: String) {
        guard arbiter.dismiss(owner: owner) else { return }
        render()
    }

    /// Mic RMS 0...1 for the waveform.
    public func level(_ rms: Double) { stage.level = rms }

    /// A host line (ADR 0221). `.system` belongs to the helper's dictation: a host `.system` show is dropped.
    public func host(_ m: HostMessage) {
        switch m {
        case .notchShow(let owner, let priority, let content, let source):
            guard priority != .system, owner != Self.dictationOwner else { logErr("notch: host system show dropped"); return }
            show(NotchEntry(owner: owner, priority: priority, content: content, source: source))
        case .notchUpdate(let owner, let content):
            guard owner != Self.dictationOwner else { return }
            update(owner: owner, content: content)
        case .notchDismiss(let owner):
            guard owner != Self.dictationOwner else { return }
            dismiss(owner: owner)
        default: break
        }
    }

    // MARK: drawing

    /// The entry to draw: the arbiter's top one, except module content while secure input is on (a password prompt).
    private func target() -> NotchEntry? {
        guard let e = arbiter.visible else { return nil }
        if e.priority == .module && IsSecureEventInputEnabled() { return nil }
        return e
    }

    private func render() {
        let t = target()
        watchSecureInput(arbiter.visible?.priority == .module)
        guard let t else {
            if drawn != nil { retract() }
            return
        }
        let wasDrawn = drawn != nil
        drawn = t
        stage.setContent(t)
        if !panel.isVisible {
            place()
            stage.expand(fromCurrent: false)
            panel.orderFrontRegardless()  // never makeKey: the frontmost app keeps focus and insertion
        } else if !wasDrawn {
            stage.expand(fromCurrent: true)  // a press mid-retract: grow back from where it is
        } else {
            stage.contentChanged()
        }
        NSAccessibility.post(element: stage, notification: .announcementRequested,
                             userInfo: [.announcement: stage.announcement, .priority: NSAccessibilityPriorityLevel.medium.rawValue])
    }

    private func retract() {
        drawn = nil
        hideGeneration += 1
        let gen = hideGeneration
        stage.retract { [weak self] in
            guard let self, self.hideGeneration == gen, self.drawn == nil else { return }  // a show took over
            self.panel.orderOut(nil)
            self.stage.running = false
        }
    }

    /// While module content holds the surface, re-check secure input twice a second (it can turn on at any time).
    private func watchSecureInput(_ on: Bool) {
        if on, secureTimer == nil {
            let t = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in
                guard let self else { return }
                let hidden = self.target() == nil
                if hidden != (self.drawn == nil) { self.render() }
            }
            RunLoop.main.add(t, forMode: .common)
            secureTimer = t
        } else if !on {
            secureTimer?.invalidate(); secureTimer = nil
        }
    }

    private func screensChanged() {
        guard panel.isVisible else { return }
        place()
        stage.snapToCurrentState()
    }

    private func place() {
        let s = Self.targetScreen()
        let ns = Self.notchScreen(s)
        let f = ns.panelFrame
        panel.setFrame(NSRect(x: f.x, y: f.y, width: f.w, height: f.h), display: false)
        stage.frame = NSRect(x: 0, y: 0, width: f.w, height: f.h)
        stage.configure(ns)
    }

    /// The screen holding the frontmost app's key (frontmost normal) window, else the main screen.
    public static func targetScreen() -> NSScreen? {
        activeScreen() ?? NSScreen.main ?? NSScreen.screens.first
    }

    /// The screen of the frontmost app's frontmost normal window (window bounds need no permission).
    public static func activeScreen() -> NSScreen? {
        guard let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier,
              let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]
        else { return nil }
        let primaryHeight = NSScreen.screens.first?.frame.height ?? 0
        for w in list where (w[kCGWindowOwnerPID as String] as? pid_t) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
            guard let b = w[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b) else { continue }
            let c = CGPoint(x: r.midX, y: primaryHeight - r.midY)  // CG (top-left) -> Cocoa (bottom-left)
            if let s = NSScreen.screens.first(where: { $0.frame.contains(c) }) { return s }
        }
        return nil
    }

    /// The notch facts of a screen. The notch spans between the two auxiliary top areas (their widths, measured
    /// from the screen's edges, so it holds whatever origin those rects use).
    static func notchScreen(_ s: NSScreen?) -> NotchScreen {
        guard let s else { return NotchScreen(frameX: 0, frameMaxY: 0, frameWidth: 0, safeTop: 0, leftMaxX: nil, rightMinX: nil) }
        let f = s.frame
        var top = 0.0, l: Double?, r: Double?
        if #available(macOS 12.0, *) {
            top = Double(s.safeAreaInsets.top)
            if let a = s.auxiliaryTopLeftArea, let b = s.auxiliaryTopRightArea {
                l = Double(f.minX + a.width); r = Double(f.maxX - b.width)
            }
        }
        return NotchScreen(frameX: Double(f.minX), frameMaxY: Double(f.maxY), frameWidth: Double(f.width), safeTop: top, leftMaxX: l, rightMinX: r)
    }
}

/// The panel's content view: the shape layer and the content row below the notch.
final class NotchStageView: NSView {
    private let shape = CAShapeLayer()
    private let row = NSView()
    private let waves = PillWave.layers.map { _ in CAShapeLayer() }
    private let waveBox = CALayer()
    private let track = CALayer()
    private let fill = CALayer()
    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    private var link: CADisplayLink?
    private var amp = PillWave.liveFloor
    private var phaseRad = 0.0
    private var last: CFTimeInterval?
    private var geo = NotchScreen(frameX: 0, frameMaxY: 0, frameWidth: 0, safeTop: 0, leftMaxX: nil, rightMinX: nil)
    private var entry: NotchEntry?
    private var extra = 0.0
    private let font: NSFont

    var level = 0.0
    var running = false {
        didSet {
            guard running != oldValue else { return }
            link?.isPaused = !running
            last = nil
        }
    }
    private(set) var announcement = ""

    init(frame: NSRect, fontName: String?) {
        let size = CGFloat(NotchGeometry.fontSize)
        font = fontName.flatMap { NSFont(name: $0, size: size) } ?? .systemFont(ofSize: size)
        super.init(frame: frame)
        wantsLayer = true
        shape.fillColor = CGColor(red: 0, green: 0, blue: 0, alpha: 1)  // black in both themes, to blend with the notch
        shape.strokeColor = nil
        layer?.addSublayer(shape)
        row.wantsLayer = true
        addSubview(row)
        waveBox.isGeometryFlipped = true  // the wave's y runs down, like the SVG
        for (i, l) in waves.enumerated() {
            l.fillColor = nil
            l.lineWidth = i == 0 ? 2 : 1.5
            l.lineCap = .round
            l.lineJoin = .round
            l.opacity = Float(PillWave.layers[i].opacity)
            waveBox.addSublayer(l)
        }
        row.layer?.addSublayer(waveBox)
        track.cornerRadius = 1.5
        fill.cornerRadius = 1.5
        track.addSublayer(fill)
        row.layer?.addSublayer(track)
        icon.symbolConfiguration = .init(pointSize: 12, weight: .regular)
        label.font = font
        label.lineBreakMode = .byTruncatingTail
        label.maximumNumberOfLines = 1
        label.cell?.truncatesLastVisibleLine = true
        label.cell?.lineBreakMode = .byTruncatingTail
        for v in [icon, label] as [NSView] { v.isHidden = true; row.addSubview(v) }
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)
        let l = displayLink(target: self, selector: #selector(tick(_:)))
        l.add(to: .main, forMode: .common)
        l.isPaused = true
        link = l
        recolor()
        drawWave()
    }

    required init?(coder: NSCoder) { nil }

    override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); recolor() }

    /// Light content on the black shape in both themes; the theme only tints the accent (the bar's fill, the alert).
    func recolor() {
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let text = NSColor(white: 0.98, alpha: 1)
        let accent = dark ? NSColor(white: 0.98, alpha: 1) : NSColor(white: 0.92, alpha: 1)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        for w in waves { w.strokeColor = text.cgColor }
        track.backgroundColor = NSColor(white: 1, alpha: 0.22).cgColor
        fill.backgroundColor = accent.cgColor
        CATransaction.commit()
        icon.contentTintColor = text
        label.textColor = text
    }

    private var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    // MARK: geometry

    func configure(_ s: NotchScreen) {
        geo = s
        layoutRow()
    }

    private var notchWidth: Double { geo.rest.width }
    private var contentTop: Double { geo.contentTop }

    /// The shape's path for a size, centered in the view, y flipped (the shared op list runs down from the top).
    private func path(width w: Double, height h: Double, radius r: Double, ear e: Double) -> CGPath {
        let ops = NotchShape.path(width: w, height: h, radius: r, ear: e)
        let x0 = (Double(bounds.width) - w) / 2, top = Double(bounds.height)
        func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: x0 + x, y: top - y) }
        let p = CGMutablePath()
        for op in ops {
            switch op {
            case .move(let x, let y): p.move(to: pt(x, y))
            case .line(let x, let y): p.addLine(to: pt(x, y))
            case .quad(let cx, let cy, let x, let y): p.addQuadCurve(to: pt(x, y), control: pt(cx, cy))
            case .close: p.closeSubpath()
            }
        }
        return p
    }

    /// At rest: exactly the hardware notch (invisible against it), or the fake tab collapsed to zero height.
    private var restPath: CGPath {
        let r = geo.rest
        return path(width: r.width, height: r.height, radius: NotchGeometry.restRadius, ear: 0)
    }

    private var expandedPath: CGPath {
        let s = NotchGeometry.expanded(notchWidth: notchWidth, notchHeight: contentTop, extra: extra)
        return path(width: s.w, height: s.h, radius: NotchGeometry.expandedRadius, ear: NotchGeometry.expandedEar)
    }

    /// The body's width now (the shape minus its ears).
    private var bodyWidth: Double { notchWidth + 2 * (NotchGeometry.sideGrow + extra) }

    // MARK: content

    func setContent(_ e: NotchEntry) {
        let changedKind = entry.map { Self.kind($0.content) != Self.kind(e.content) } ?? true
        entry = e
        announcement = e.source.map { "\($0): \(e.content.accessibilityLabel)" } ?? e.content.accessibilityLabel
        setAccessibilityLabel(announcement)
        if changedKind, case .level = e.content { amp = PillWave.target(waveMode, 0); drawWave() }
        layoutRow()
        running = true
    }

    private static func kind(_ c: NotchContent) -> Int {
        switch c {
        case .level: 0
        case .status: 1
        case .progress: 2
        case .icon: 3
        }
    }

    private var waveMode: PillWave.Mode {
        if case .level(let busy) = entry?.content, !busy { return .live }
        return .idle
    }

    /// The label's text: the module's registered name first (attribution, ADR 0221), dimmed.
    private func text(_ s: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        if let src = entry?.source {
            out.append(NSAttributedString(string: src + "  ", attributes: [.font: font, .foregroundColor: NSColor(white: 1, alpha: 0.55)]))
        }
        out.append(NSAttributedString(string: s, attributes: [.font: font, .foregroundColor: NSColor(white: 0.98, alpha: 1)]))
        return out
    }

    /// Lays the row out below the notch (never under it), centered, and picks the widening the content needs.
    private func layoutRow() {
        let rowH = NotchGeometry.dropGrow
        let rowY = Double(bounds.height) - contentTop - rowH
        row.frame = NSRect(x: 0, y: rowY, width: Double(bounds.width), height: rowH)
        let midY = rowH / 2, gap = NotchGeometry.gap, iconS = NotchGeometry.iconSize
        let maxBody = notchWidth + 2 * (NotchGeometry.sideGrow + NotchGeometry.widenMax)
        let maxText = maxBody - 2 * NotchGeometry.padding
        var wantW = 0.0
        CATransaction.begin(); CATransaction.setDisableActions(true)
        waveBox.isHidden = true; track.isHidden = true; icon.isHidden = true; label.isHidden = true
        switch entry?.content {
        case .level?, nil:
            waveBox.isHidden = false
            wantW = PillWave.width
            waveBox.frame = CGRect(x: (Double(bounds.width) - PillWave.width) / 2, y: midY - PillWave.height / 2,
                                   width: PillWave.width, height: PillWave.height)
        case .status(let t, let tone)?:
            let alert = tone == .error
            let lead = alert ? iconS + gap : 0
            let tw = fitLabel(text(t), max: maxText - lead)
            wantW = lead + tw
            let x0 = (Double(bounds.width) - wantW) / 2
            if alert { placeIcon("exclamationmark.triangle", x: x0, midY: midY) }
            placeLabel(x: x0 + lead, w: tw, midY: midY)
        case .icon(let i, let l)?:
            let lead = iconS + gap
            let tw = fitLabel(text(l), max: maxText - lead)
            wantW = lead + tw
            let x0 = (Double(bounds.width) - wantW) / 2
            placeIcon(i.symbol, x: x0, midY: midY)
            placeLabel(x: x0 + lead, w: tw, midY: midY)
        case .progress(let l, let v)?:
            let barW = 64.0
            let tw = fitLabel(text(l), max: maxText - barW - gap)
            wantW = tw + gap + barW
            let x0 = (Double(bounds.width) - wantW) / 2
            placeLabel(x: x0, w: tw, midY: midY)
            track.isHidden = false
            track.frame = CGRect(x: x0 + tw + gap, y: midY - 1.5, width: barW, height: 3)
            fill.removeAnimation(forKey: "notch.indeterminate")
            if let v {
                fill.frame = CGRect(x: 0, y: 0, width: barW * max(0, min(1, v)), height: 3)
            } else {
                fill.frame = CGRect(x: 0, y: 0, width: barW * 0.3, height: 3)
                if !reduceMotion {
                    let slide = CABasicAnimation(keyPath: "position.x")
                    slide.fromValue = barW * 0.15; slide.toValue = barW * 0.85
                    slide.duration = 0.9; slide.autoreverses = true; slide.repeatCount = .infinity
                    slide.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
                    fill.add(slide, forKey: "notch.indeterminate")
                }
            }
        }
        CATransaction.commit()
        extra = NotchGeometry.extra(notchWidth: notchWidth, contentWidth: wantW)
    }

    /// The label's width: its natural width, capped (the tail truncates with an ellipsis).
    private func fitLabel(_ s: NSAttributedString, max cap: Double) -> Double {
        label.attributedStringValue = s
        label.sizeToFit()
        return min(Double(label.frame.width), Swift.max(0, cap))
    }

    private func placeLabel(x: Double, w: Double, midY: Double) {
        let h = Double(label.frame.height)
        label.frame = NSRect(x: x, y: midY - h / 2, width: w, height: h)
        label.isHidden = false
    }

    private func placeIcon(_ symbol: String, x: Double, midY: Double) {
        let s = NotchGeometry.iconSize
        icon.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        icon.frame = NSRect(x: x, y: midY - s / 2, width: s, height: s)
        icon.isHidden = false
    }

    // MARK: motion (Core Animation only; the panel's frame never animates)

    private var generation = 0

    private func presentedPath() -> CGPath? { (shape.presentation() ?? shape).path }
    private func presentedOpacity(_ l: CALayer) -> Float { (l.presentation() ?? l).opacity }

    /// Show: the shape springs from the rest rect (or from where a retract left it) to the expanded size on a system
    /// spring with a `.bouncy` feel (perceptual 0.35 s, bounce 0.25); the content fades in as the shape arrives
    /// (from 0.13 s over 0.10 s). The overshoot (~3% of the travel: about 2 pt each side, 1-2 pt down) stays inside
    /// the panel, which has `widenMax` + ear spare each side and 4 pt below (`NotchGeometry.panelSize`), so nothing
    /// clips and it reaches no further over the menu bar than the panel already does. Reduce Motion: a 120 ms fade.
    func expand(fromCurrent: Bool) {
        generation += 1
        guard let root = layer, let rowLayer = row.layer else { return }
        let to = expandedPath
        shape.removeAllAnimations(); rowLayer.removeAllAnimations(); root.removeAllAnimations()
        if reduceMotion {
            let o0: Float = fromCurrent ? presentedOpacity(root) : 0
            CATransaction.begin(); CATransaction.setDisableActions(true)
            shape.path = to; rowLayer.opacity = 1; root.opacity = 1
            CATransaction.commit()
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = o0; fade.toValue = 1; fade.duration = 0.12
            root.add(fade, forKey: "notch.fade")
            return
        }
        let from = fromCurrent ? (presentedPath() ?? restPath) : restPath
        let c0: Float = fromCurrent ? presentedOpacity(rowLayer) : 0
        CATransaction.begin(); CATransaction.setDisableActions(true)
        root.opacity = 1; shape.path = to; rowLayer.opacity = 1
        CATransaction.commit()
        let grow = CASpringAnimation(perceptualDuration: 0.35, bounce: 0.25)
        grow.keyPath = "path"
        grow.fromValue = from; grow.toValue = to
        grow.duration = grow.settlingDuration
        shape.add(grow, forKey: "notch.path")
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = c0; fade.toValue = 1
        fade.beginTime = CACurrentMediaTime() + (fromCurrent ? 0.04 : 0.13)
        fade.duration = 0.10
        fade.fillMode = .backwards
        fade.timingFunction = CAMediaTimingFunction(name: .easeOut)
        rowLayer.add(fade, forKey: "notch.content")
    }

    /// Hide: the content fades out, then the shape retracts into the rest rect on a no-bounce system spring
    /// (`.smooth`'s bounce 0 at a 0.3 s perceptual duration, so nothing pokes back out of the notch); `done` unless a
    /// show took over.
    func retract(_ done: @escaping () -> Void) {
        generation += 1
        let gen = generation
        guard let root = layer, let rowLayer = row.layer else { done(); return }
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.generation == gen else { return }
            done()
        }
        if reduceMotion {
            let o0 = presentedOpacity(root)
            root.removeAllAnimations()
            CATransaction.begin(); CATransaction.setDisableActions(true); root.opacity = 0; CATransaction.commit()
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = o0; fade.toValue = 0; fade.duration = 0.12
            root.add(fade, forKey: "notch.fade")
        } else {
            let from = presentedPath() ?? expandedPath
            let c0 = presentedOpacity(rowLayer)
            shape.removeAllAnimations(); rowLayer.removeAllAnimations()
            let to = restPath
            CATransaction.begin(); CATransaction.setDisableActions(true)
            shape.path = to; rowLayer.opacity = 0
            CATransaction.commit()
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = c0; fade.toValue = 0; fade.duration = 0.06
            fade.timingFunction = CAMediaTimingFunction(name: .easeIn)
            rowLayer.add(fade, forKey: "notch.content")
            let shrink = CASpringAnimation(perceptualDuration: 0.3, bounce: 0)
            shrink.keyPath = "path"
            shrink.fromValue = from; shrink.toValue = to
            shrink.beginTime = CACurrentMediaTime() + 0.04
            shrink.duration = shrink.settlingDuration
            shrink.fillMode = .backwards
            shape.add(shrink, forKey: "notch.path")
        }
        CATransaction.commit()
    }

    /// A content change while shown: a short content fade, and a small spring if the width changed.
    func contentChanged() {
        guard let rowLayer = row.layer else { return }
        let fade = CATransition()
        fade.type = .fade
        fade.duration = reduceMotion ? 0.08 : 0.12
        rowLayer.add(fade, forKey: "notch.swap")
        let to = expandedPath
        guard let from = presentedPath() else { return }
        CATransaction.begin(); CATransaction.setDisableActions(true); shape.path = to; CATransaction.commit()
        guard !reduceMotion else { return }
        let widen = CASpringAnimation(perceptualDuration: 0.2, bounce: 0.04)
        widen.keyPath = "path"
        widen.fromValue = from; widen.toValue = to
        widen.duration = widen.settlingDuration
        shape.add(widen, forKey: "notch.path")
    }

    /// The screen changed under a shown surface: redraw the current state at once, no motion.
    func snapToCurrentState() {
        guard let rowLayer = row.layer else { return }
        shape.removeAllAnimations(); rowLayer.removeAllAnimations()
        CATransaction.begin(); CATransaction.setDisableActions(true)
        shape.path = entry == nil ? restPath : expandedPath
        CATransaction.commit()
    }

    // MARK: the wave

    @objc private func tick(_ l: CADisplayLink) {
        let now = l.timestamp
        let dtMs = last.map { (now - $0) * 1000 } ?? 16
        last = now
        let mode = waveMode
        amp = PillWave.step(amp, PillWave.target(mode, mode == .live ? level : 0), dtMs)
        if !reduceMotion {
            phaseRad = (phaseRad + PillWave.speed(mode) * dtMs / 1000).truncatingRemainder(dividingBy: Double.pi * 200)
        }
        drawWave()
    }

    private func drawWave() {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        for (layer, pts) in zip(waves, PillWave.frame(amp: amp, phase: phaseRad)) {
            let p = CGMutablePath()
            for (i, q) in pts.enumerated() {
                let pt = CGPoint(x: q.x, y: q.y)
                if i == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
            }
            layer.path = p
        }
        CATransaction.commit()
    }
}
#endif
