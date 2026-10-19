#if os(macOS)
// ctrl's read-only Mac check (run 15; ctrl do, ADR 0215). Built by scripts/ctrl/mac-check.sh together with a copy of
// QrtzCtrl's own sources, so every read goes through the code the helper ships. Strictly read-only: no activation, no
// keystroke, no press, no window change, no event tap, nothing that can show a permission dialog (Accessibility
// trust is read with prompt = false). It reads ctrl do's tree and menu shortcuts of the frontmost app and prints only
// counts: nodes, depth, roles, how many text inputs carry a value, how many shortcuts. Never a title or a value.
// Run 47: if qrtz installed model.laya, its fixed first questions are asked through the shipped Core ML runtime.
import AppKit
import ApplicationServices
import Foundation

let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
let axTrusted = AXIsProcessTrustedWithOptions([promptKey: false] as CFDictionary)
let backend = MacCtrlBackend()
let frontBefore = NSWorkspace.shared.frontmostApplication?.bundleIdentifier

var out: [String: Any] = ["os": ProcessInfo.processInfo.operatingSystemVersionString, "ax_trusted": axTrusted]
if let c = try? backend.catalog() {
    out["catalog"] = ["apps": c.apps.count, "running": c.apps.filter(\.running).count, "browsers": c.apps.filter(\.browser).count,
                      "default_browser": c.defaultBrowser != nil]
}
if axTrusted, let front = frontBefore {
    do {
        let t0 = Date()
        let t = try backend.tree(app: front, scope: nil, depth: 6)
        let all = t.root.all
        var roles: [String: Int] = [:]
        for n in all { roles[n.role, default: 0] += 1 }
        out["tree"] = ["nodes": all.count, "ms": Int(Date().timeIntervalSince(t0) * 1000), "focused": t.focused != nil,
                       "more": all.filter { $0.more > 0 }.count, "with_value": all.filter { $0.value != nil }.count,
                       "secure_seen": all.filter { CtrlTreeRules.secureRoles.contains($0.role) }.count, "roles": roles]
    } catch let f as CtrlFailure {
        out["tree"] = f.code
    } catch {
        out["tree"] = "error"
    }
    do {
        let m = try backend.menuShortcuts(app: front)
        out["menu_shortcuts"] = ["items": m.count, "enabled": m.filter(\.enabled).count]
    } catch let f as CtrlFailure {
        out["menu_shortcuts"] = f.code
    } catch {
        out["menu_shortcuts"] = "error"
    }
} else {
    out["tree"] = "not trusted"
}

// Run 47: the first real Laya questions, when qrtz has installed model.laya with ctrl (or QRTZ_CTRL_LAYA_PACKAGE
// names a package). Read-only: qrtz's own store, the shipped runtime's as-is load (nothing compiled or written into
// the store), no network, no UI. Nothing installed → "installed": false, nothing loaded.
var laya: [String: Any] = [:]
let env = ProcessInfo.processInfo.environment
let home = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(CtrlLayaFirstQuestion.dataDir).path
if let pkg = env["QRTZ_CTRL_LAYA_PACKAGE"].flatMap({ $0.isEmpty ? nil : $0 }) ?? CtrlLayaFirstQuestion.installedModel(root: home) {
    laya["installed"] = true
    laya["model"] = URL(fileURLWithPath: pkg).lastPathComponent
    laya["version"] = URL(fileURLWithPath: pkg).deletingLastPathComponent().lastPathComponent
    switch CtrlLayaLoad.plan(path: pkg, fromHost: true) {
    case let .refuse(code): laya["load"] = code
    case .compile: laya["load"] = "laya.needs_compiled"
    case .asIs:
        laya["load"] = "as_is"
        var answers: [[String: Any]] = []
        for q in CtrlLayaFirstQuestion.questions {
            let t0 = Date()
            var p: [Double]?, err: String?
            do { p = try backend.laya(model: pkg, state: q.state, question: q.question, options: q.options) } catch let f as CtrlFailure {
                err = f.code
            } catch { err = "laya.load_failed" }
            answers.append(CtrlLayaFirstQuestion.report(q, probabilities: p, error: err, ms: Int(Date().timeIntervalSince(t0) * 1000)))
        }
        laya["answers"] = answers // the first one's ms includes the Core ML load
    }
} else {
    laya["installed"] = false
}
out["laya"] = laya

out["frontmost_unchanged"] = NSWorkspace.shared.frontmostApplication?.bundleIdentifier == frontBefore
let data = try JSONSerialization.data(withJSONObject: out, options: [.prettyPrinted, .sortedKeys])
print(String(decoding: data, as: UTF8.self))
#endif
