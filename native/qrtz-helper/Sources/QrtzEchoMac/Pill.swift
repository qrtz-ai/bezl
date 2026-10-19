#if os(macOS)
import AppKit
import CoreText
import QrtzEcho
import QrtzMacShared
import QrtzShared

/// ADR 0092 / ADR 0218: echo's dictation pill is one client of the shared notch surface (QrtzMacShared
/// NotchSurface): listening draws the waveform, processing its slow swell, an error or notice (and ctrl+fn's
/// "ctrl: …") a line of text, all below the notch at the top center of the active screen. The old bottom pill is
/// gone. The surface is built at launch (font registered, layers made), so the first fn press only orders it front.
/// Host `notch.*` lines (ctrl run status, module requests the host vetted) draw on the same surface, under dictation.
///
/// ADR 0211: qrtz's theme setting (dark, light or system) is set on the panel and followed live (`PillThemeWatch`).
/// The shape stays black in both, to blend with the notch; the theme only reaches the content's accent.
public final class NativePill: PillView, @unchecked Sendable {
    private var surface: NotchSurface?
    private var themeWatch: PillThemeWatch?
    /// The theme the panel draws (nil: the host named none, so macOS's appearance, as before ADR 0211).
    public private(set) var theme: PillTheme?

    /// `themeFile`: the host's theme file (`QRTZ_PILL_THEME_FILE`); nil: none, the pill follows macOS.
    public init(themeFile: String? = PillTheme.file()) {
        if Thread.isMainThread { build(themeFile: themeFile) } else { DispatchQueue.main.sync { build(themeFile: themeFile) } }
    }

    /// qrtz's theme setting, now (also while the pill is up).
    public func setTheme(_ t: PillTheme) { DispatchQueue.main.async { [self] in applyTheme(t) } }

    private func applyTheme(_ t: PillTheme?) {
        theme = t
        // Explicit on the panel: dark or light whatever macOS uses; system (nil) inherits macOS's appearance.
        surface?.appearance = t?.appearance
    }

    public func show(_ phase: PillPhase) { DispatchQueue.main.async { [self] in apply(phase) } }
    public func level(_ rms: Double) { DispatchQueue.main.async { [self] in surface?.level(rms) } }
    public func notch(_ m: HostMessage) { DispatchQueue.main.async { [self] in surface?.host(m) } }

    public static func registerFont() {
        let data = PillFont.woff2 as CFData
        guard let provider = CGDataProvider(data: data), let font = CGFont(provider) else { logErr("pill: font unreadable"); return }
        var err: Unmanaged<CFError>?
        if !CTFontManagerRegisterGraphicsFont(font, &err) {
            // Already registered (a second build in one process) is fine; anything else falls back to the system font.
            if let e = err?.takeRetainedValue(), CFErrorGetCode(e) != CTFontManagerError.alreadyRegistered.rawValue { logErr("pill: font: \(e)") }
        }
    }

    private func build(themeFile: String?) {
        // A window needs the app object; accessory: no Dock icon, no menu bar, never frontmost.
        let app = NSApplication.shared
        if app.activationPolicy() != .accessory { app.setActivationPolicy(.accessory) }
        Self.registerFont()
        surface = NotchSurface(fontName: PillFont.postScriptName)
        if let f = themeFile {
            let initial = PillTheme.read(path: f)
            applyTheme(initial)
            themeWatch = PillThemeWatch(path: f, initial: initial) { [weak self] t in self?.applyTheme(t) }
            if themeWatch == nil { logErr("pill: theme folder unreadable; the pill keeps its launch theme") }
        }
    }

    /// Dictation's content on the surface (owner `echo.dictation`, priority system).
    static func content(_ phase: PillPhase) -> NotchContent? {
        switch phase {
        case .hidden: nil
        case .listening: .level(busy: false)
        case .processing: .level(busy: true)
        case .error(let m): .status(text: m, tone: .error)
        case .info(let m): .status(text: m, tone: .normal)
        }
    }

    private func apply(_ phase: PillPhase) {
        guard let s = surface else { return }
        if let c = Self.content(phase) {
            s.show(NotchEntry(owner: NotchSurface.dictationOwner, priority: .system, content: c))
        } else {
            s.dismiss(owner: NotchSurface.dictationOwner)
        }
    }
}
#endif
