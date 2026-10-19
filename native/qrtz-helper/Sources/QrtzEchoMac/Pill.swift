#if os(macOS)
import AppKit
import CoreText
import QrtzEcho
import QrtzMacShared
import QrtzShared
import QuartzCore

/// ADR 0092: echo's dictation pill as a native panel. It doesn't activate, is borderless, ignores the mouse, shows on
/// every Space and over full-screen apps, and sits bottom-center on the screen of the active window. It is built at
/// launch (font registered, layers made), so the first fn press only orders it front. 62b99fc's design: never any
/// transcript; a 240 pt pill; three layered tapered sines eased to the mic level every display frame; a slow low
/// swell while processing; an alert icon and lowercase error text truncated with an ellipsis. With reduced motion
/// the wave's phase stands still and only the amplitude follows the level. `PillDriver` (core) decides the phase.
public final class NativePill: PillView, @unchecked Sendable {
    private var panel: NSPanel?
    private var view: PillContentView?

    public init() {
        if Thread.isMainThread { build() } else { DispatchQueue.main.sync { build() } }
    }

    public func show(_ phase: PillPhase) { DispatchQueue.main.async { [self] in apply(phase) } }
    public func level(_ rms: Double) { DispatchQueue.main.async { [self] in view?.level = rms } }

    public static func registerFont() {
        let data = PillFont.woff2 as CFData
        guard let provider = CGDataProvider(data: data), let font = CGFont(provider) else { logErr("pill: font unreadable"); return }
        var err: Unmanaged<CFError>?
        if !CTFontManagerRegisterGraphicsFont(font, &err) {
            // Already registered (a second build in one process) is fine; anything else falls back to the system font.
            if let e = err?.takeRetainedValue(), CFErrorGetCode(e) != CTFontManagerError.alreadyRegistered.rawValue { logErr("pill: font: \(e)") }
        }
    }

    private func build() {
        // A window needs the app object; accessory: no Dock icon, no menu bar, never frontmost.
        let app = NSApplication.shared
        if app.activationPolicy() != .accessory { app.setActivationPolicy(.accessory) }
        Self.registerFont()
        let p = NSPanel(contentRect: NSRect(x: 0, y: 0, width: PillGeometry.width, height: PillGeometry.height),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        p.isFloatingPanel = true
        p.level = .statusBar
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        p.ignoresMouseEvents = true
        p.hidesOnDeactivate = false
        p.becomesKeyOnlyIfNeeded = true
        p.isOpaque = false
        p.backgroundColor = .clear
        p.hasShadow = false
        p.isReleasedWhenClosed = false
        p.animationBehavior = .none
        let v = PillContentView(frame: p.contentLayoutRect)
        p.contentView = v
        panel = p; view = v
    }

    private func apply(_ phase: PillPhase) {
        guard let p = panel, let v = view else { return }
        let wasHidden = v.phase == .hidden
        v.phase = phase
        if phase == .hidden {
            // Out: shrink and fade while insertion runs; orderOut lands when it ends (never eats clicks after).
            guard p.isVisible, !wasHidden else { p.orderOut(nil); v.running = false; return }
            v.animateOut { [weak p, weak v] in
                guard let p, let v, v.phase == .hidden else { return }  // a new press took over mid-fade
                p.orderOut(nil)
                v.running = false
            }
            return
        }
        if !p.isVisible {
            place(p)
            v.resetAmp()
            v.animateIn(fromCurrent: false)
            p.orderFrontRegardless()
        } else if wasHidden {
            v.animateIn(fromCurrent: true)  // pressed again mid-fade-out: grow back from where it is
        } else {
            v.crossfadeContent()
        }
        v.running = true
        NSAccessibility.post(element: v, notification: .announcementRequested,
                             userInfo: [.announcement: phase.accessibilityLabel, .priority: NSAccessibilityPriorityLevel.medium.rawValue])
    }

    private func place(_ p: NSPanel) {
        let vf = (Self.activeScreen() ?? NSScreen.main ?? NSScreen.screens.first)?.visibleFrame ?? .zero
        let f = PillGeometry.frame(visibleX: vf.minX, y: vf.minY, width: vf.width, height: vf.height)
        p.setFrame(NSRect(x: f.x, y: f.y, width: f.w, height: f.h), display: false)
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
}

/// The pill's drawing: a rounded surface with a 1 pt border, the wave (three CAShapeLayers), or the error row.
public final class PillContentView: NSView {
    private let surface = CAShapeLayer()
    private let waves = PillWave.layers.map { _ in CAShapeLayer() }
    private let waveBox = CALayer()
    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    private var link: CADisplayLink?
    private var amp = PillWave.liveFloor
    private var phaseRad = 0.0
    private var last: CFTimeInterval?

    public var level = 0.0
    public var phase: PillPhase = .hidden { didSet { layoutPhase() } }
    public var running = false {
        didSet {
            guard running != oldValue else { return }
            link?.isPaused = !running
            last = nil
        }
    }

    public override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.addSublayer(surface)
        waveBox.frame = CGRect(x: (frame.width - PillWave.width) / 2, y: (frame.height - PillWave.height) / 2,
                               width: PillWave.width, height: PillWave.height)
        waveBox.isGeometryFlipped = true  // the wave's y runs down, like the SVG
        for (i, l) in waves.enumerated() {
            l.fillColor = nil
            l.lineWidth = i == 0 ? 2 : 1.5
            l.lineCap = .round
            l.lineJoin = .round
            l.opacity = Float(PillWave.layers[i].opacity)
            waveBox.addSublayer(l)
        }
        layer?.addSublayer(waveBox)
        icon.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: nil)
        icon.symbolConfiguration = .init(pointSize: 12, weight: .regular)
        label.font = NSFont(name: PillFont.postScriptName, size: PillGeometry.fontSize) ?? .systemFont(ofSize: PillGeometry.fontSize)
        label.lineBreakMode = .byTruncatingTail
        label.maximumNumberOfLines = 1
        label.cell?.truncatesLastVisibleLine = true
        for v in [icon, label] as [NSView] { v.isHidden = true; addSubview(v) }
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)
        setAccessibilityLabel(PillPhase.hidden.accessibilityLabel)
        let l = displayLink(target: self, selector: #selector(tick(_:)))
        l.add(to: .main, forMode: .common)
        l.isPaused = true
        link = l
        recolor()
        draw(amp: amp)
    }

    public required init?(coder: NSCoder) { nil }

    public override var isFlipped: Bool { false }

    public override func viewDidChangeEffectiveAppearance() { super.viewDidChangeEffectiveAppearance(); recolor() }

    /// packages/ui tokens: surface, borderStrong and text, dark and light.
    private func recolor() {
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        func c(_ hex: UInt32) -> CGColor {
            CGColor(red: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
        let text = c(dark ? 0xfafafa : 0x0a0a0a)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        surface.fillColor = c(dark ? 0x111111 : 0xfafafa)
        surface.strokeColor = c(dark ? 0x404040 : 0xd4d4d4)
        surface.lineWidth = 1
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        surface.path = CGPath(roundedRect: r, cornerWidth: r.height / 2, cornerHeight: r.height / 2, transform: nil)
        for w in waves { w.strokeColor = text }
        CATransaction.commit()
        icon.contentTintColor = NSColor(cgColor: text)
        label.textColor = NSColor(cgColor: text)
    }

    // MARK: appear / disappear (Core Animation on the content layer; never awaited by capture or insertion)

    private static let inScale: CGFloat = 0.86
    private static let outScale: CGFloat = 0.9
    private var outGeneration = 0

    private var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    /// A scale about the view's center (the backing layer's anchor is its bottom-left corner).
    private func centered(_ k: CGFloat) -> CATransform3D {
        let cx = bounds.midX, cy = bounds.midY
        var t = CATransform3DMakeTranslation(cx, cy, 0)
        t = CATransform3DScale(t, k, k, 1)
        return CATransform3DTranslate(t, -cx, -cy, 0)
    }

    private func current() -> (CATransform3D, Float) {
        guard let l = layer else { return (CATransform3DIdentity, 1) }
        let pl = l.presentation() ?? l
        return (pl.transform, pl.opacity)
    }

    /// In: a near-critically-damped spring from a small scale and 0 opacity (about 220 ms); a plain fade with Reduce Motion.
    public func animateIn(fromCurrent: Bool) {
        guard let l = layer else { return }
        outGeneration += 1
        let (t0, o0) = fromCurrent ? current() : (centered(Self.inScale), 0)
        l.removeAllAnimations()
        CATransaction.begin(); CATransaction.setDisableActions(true)
        l.transform = CATransform3DIdentity
        l.opacity = 1
        CATransaction.commit()
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = o0; fade.toValue = 1
        fade.duration = reduceMotion ? 0.12 : 0.16
        fade.timingFunction = CAMediaTimingFunction(name: .easeOut)
        l.add(fade, forKey: "pill.opacity")
        guard !reduceMotion else { return }
        let grow = CASpringAnimation(perceptualDuration: 0.22, bounce: 0.04)
        grow.keyPath = "transform"
        grow.fromValue = NSValue(caTransform3D: t0)
        grow.toValue = NSValue(caTransform3D: CATransform3DIdentity)
        grow.duration = grow.settlingDuration
        l.add(grow, forKey: "pill.transform")
    }

    /// Out: shrink and fade (about 170 ms), then `done` unless a press cancelled it.
    public func animateOut(_ done: @escaping () -> Void) {
        guard let l = layer else { done(); return }
        outGeneration += 1
        let gen = outGeneration
        let (t0, o0) = current()
        l.removeAllAnimations()
        let dur = reduceMotion ? 0.12 : 0.17
        let tEnd = reduceMotion ? CATransform3DIdentity : centered(Self.outScale)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        l.transform = tEnd
        l.opacity = 0
        CATransaction.commit()
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.outGeneration == gen else { return }
            done()
        }
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = o0; fade.toValue = 0
        fade.duration = dur
        fade.timingFunction = CAMediaTimingFunction(name: .easeIn)
        l.add(fade, forKey: "pill.opacity")
        if !reduceMotion {
            let shrink = CABasicAnimation(keyPath: "transform")
            shrink.fromValue = NSValue(caTransform3D: t0)
            shrink.toValue = NSValue(caTransform3D: tEnd)
            shrink.duration = dur
            shrink.timingFunction = CAMediaTimingFunction(controlPoints: 0.4, 0, 1, 1)
            l.add(shrink, forKey: "pill.transform")
        }
        CATransaction.commit()
    }

    /// A phase change while shown (listening -> processing -> error text): a quick crossfade plus a hair of spring.
    public func crossfadeContent() {
        guard let l = layer else { return }
        let fade = CATransition()
        fade.type = .fade
        fade.duration = reduceMotion ? 0.1 : 0.14
        l.add(fade, forKey: "pill.content")
        guard !reduceMotion, l.animation(forKey: "pill.transform") == nil else { return }
        let pop = CASpringAnimation(perceptualDuration: 0.2, bounce: 0.04)
        pop.keyPath = "transform"
        pop.fromValue = NSValue(caTransform3D: centered(0.97))
        pop.toValue = NSValue(caTransform3D: CATransform3DIdentity)
        pop.duration = pop.settlingDuration
        l.add(pop, forKey: "pill.transform")
    }

    public func resetAmp() {
        amp = PillWave.target(mode, phase == .listening ? level : 0)
        draw(amp: amp)
    }

    private var mode: PillWave.Mode { phase == .listening ? .live : .idle }

    private func layoutPhase() {
        setAccessibilityLabel(phase.accessibilityLabel)
        let words: (String, Bool)? = {
            switch phase {
            case .error(let m): return (m, true)
            case .info(let m): return (m, false)
            default: return nil
            }
        }()
        if let (message, alert) = words {
            // An error: the alert icon and the text; a notice ("copied to clipboard"): the text alone.
            waveBox.isHidden = true
            label.stringValue = message
            label.sizeToFit()
            let iconW = alert ? PillGeometry.iconSize + PillGeometry.gap : 0
            let tw = min(label.frame.width, PillGeometry.errorTextWidth + (alert ? 0 : PillGeometry.iconSize + PillGeometry.gap))
            let x0 = (bounds.width - iconW - tw) / 2
            icon.frame = NSRect(x: x0, y: (bounds.height - PillGeometry.iconSize) / 2, width: PillGeometry.iconSize, height: PillGeometry.iconSize)
            label.frame = NSRect(x: x0 + iconW, y: (bounds.height - label.frame.height) / 2, width: tw, height: label.frame.height)
            icon.isHidden = !alert; label.isHidden = false
        } else {
            waveBox.isHidden = false
            icon.isHidden = true; label.isHidden = true
        }
    }

    @objc private func tick(_ l: CADisplayLink) {
        let now = l.timestamp
        let dtMs = last.map { (now - $0) * 1000 } ?? 16
        last = now
        amp = PillWave.step(amp, PillWave.target(mode, phase == .listening ? level : 0), dtMs)
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            phaseRad = (phaseRad + PillWave.speed(mode) * dtMs / 1000).truncatingRemainder(dividingBy: Double.pi * 200)
        }
        draw(amp: amp)
    }

    private func draw(amp: Double) {
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
