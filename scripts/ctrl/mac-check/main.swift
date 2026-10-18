#if os(macOS)
// ctrl's read-only Mac check (run 15). Built by scripts/ctrl/mac-check.sh together with a copy of QrtzCtrl's own
// sources, so every read goes through the code the helper ships. Strictly read-only (ADR 0100): no activation, no
// keystroke, no click, no window change, and nothing that can show a permission dialog. Accessibility trust is
// read with prompt = false; Apple Events consent with askUserIfNeeded = false, and a browser's tabs are read only
// where that consent is already granted (titles and URLs only, without the page script; nothing but counts is
// printed). No protected folder (Desktop, Downloads, Documents, ~/.Trash) is touched: that could prompt.
import AppKit
import ApplicationServices
import Foundation

func uiState() -> [String: Any] {
    let s = CtrlProbe.snapshot()
    return [
        "frontmost_app": NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? NSNull(),
        "frontmost_pid": s.frontmostPid.map { NSNumber(value: $0) } ?? NSNull(),
        "cursor": s.cursor.map { ["x": $0.x, "y": $0.y] } ?? NSNull(),
        "key_window": s.keyWindow.map { NSNumber(value: $0) } ?? NSNull(),
        "focused_element": s.focusedElement ?? NSNull(),
    ]
}

let before = CtrlProbe.snapshot()
let beforeUi = uiState()
let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
let axTrusted = AXIsProcessTrustedWithOptions([promptKey: false] as CFDictionary)

var out: [String: Any] = ["os": ProcessInfo.processInfo.operatingSystemVersionString]
let backend = MacCtrlBackend()
if let cat = try? backend.catalog() {
    out["catalog_count"] = cat.apps.count
    out["catalog_browsers"] = cat.apps.filter(\.browser).map(\.bundleId)
    out["catalog_running"] = cat.apps.filter(\.running).count
    out["default_browser"] = cat.defaultBrowser ?? NSNull()
} else {
    out["catalog_count"] = NSNull()
}
out["ax_trusted"] = axTrusted
out["post_event"] = CtrlProbe.postEventAllowed
out["secure_input"] = CtrlProbe.secureInput

let english = Locale.preferredLanguages.first?.lowercased().hasPrefix("en") ?? false
var browsers: [String: Any] = [:]
let ids = ["com.apple.Safari", "com.google.Chrome", "com.google.Chrome.beta", "com.google.Chrome.canary",
           "org.chromium.Chromium", "com.brave.Browser", "com.microsoft.edgemac", "com.vivaldi.Vivaldi"]
for id in ids {
    guard let flavor = CtrlTabFlavor.of(id) else { continue }
    guard let pid = backend.pid(of: id) else {
        if id == "com.apple.Safari" || id == "com.google.Chrome" { browsers[id] = ["running": false] }
        continue
    }
    var b: [String: Any] = ["running": true]
    // Window count from AX (no Apple Event); nil without the AX grant.
    if axTrusted { b["ax_windows"] = CtrlActions.windows(pid).count }
    let consent = MacCtrlBackend.automation(id) ?? "unknown"
    b["automation"] = consent
    if consent == "granted", let src = CtrlTabs.enumerateSource(bundleId: id, flavor: flavor, withPageScript: false) {
        if let text = try? MacCtrlBackend.runScript(src).stringValue, let rows = CtrlTabs.parse(text) {
            let strips = axTrusted ? CtrlActions.windows(pid).flatMap(MacCtrlBackend.tabStrips) : []
            let tabs = CtrlTabs.join(rows, strips: strips, flavor: flavor, english: english)
            b["windows"] = Set(rows.map(\.windowId)).count
            b["tabs"] = rows.count
            b["blank_tabs"] = tabs.filter(\.blank).count
            b["ax_strips"] = strips.count
            b["ax_strip_tabs"] = strips.map(\.count)
            b["pinned_true"] = tabs.filter { $0.pinned == true }.count
            b["pinned_false"] = tabs.filter { $0.pinned == false }.count
            b["pinned_unknown"] = tabs.filter { $0.pinned == nil }.count
            if flavor == .safari {
                var rd: [String: Int] = [:]
                for t in strips.joined() { rd[t.roleDescription ?? "nil", default: 0] += 1 }
                b["ax_role_descriptions"] = rd
            } else {
                b["ax_names_with_pinned_marker"] = strips.joined().filter { t in
                    CtrlTabs.chromiumPinned.contains { t.name.lowercased().contains($0) }
                }.count
            }
        } else {
            b["tabs"] = "unreadable"
        }
    } else {
        b["tabs"] = "not granted"
    }
    browsers[id] = b
}
out["browsers"] = browsers

let after = CtrlProbe.snapshot()
out["guard_before"] = beforeUi
out["guard_after"] = uiState()
out["guard_unchanged"] = before.frontmostPid == after.frontmostPid && before.cursor == after.cursor
    && before.focusedElement == after.focusedElement && before.keyWindow == after.keyWindow
out["guard_fields"] = [
    "frontmost": before.frontmostPid == after.frontmostPid, "cursor": before.cursor == after.cursor,
    "focused_element": before.focusedElement == after.focusedElement, "key_window": before.keyWindow == after.keyWindow,
]
let data = try JSONSerialization.data(withJSONObject: out, options: [.prettyPrinted, .sortedKeys])
print(String(decoding: data, as: UTF8.self))
#endif
