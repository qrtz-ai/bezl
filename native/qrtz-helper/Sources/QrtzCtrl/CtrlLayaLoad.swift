// How the Core ML runtime may open a Laya model (ADR 0157 §9, run 36). Foundation only, so the rule is tested on
// Linux; the macOS runtime (CtrlBackendMac's CtrlLayaRuntime) asks `plan` before every first load of a path.
//
// A path the host sends with `ctrl.laya` / `ctrl.laya.warm` is the store-linked `model.laya` package (the time-boxed
// `extension.reads` exception): it is opened read-only, as is, and nothing is ever written next to it. So from the
// host only an already compiled `.mlmodelc` loads, and only when no file in it is writable by anyone (the store links
// 0444/0400; the host re-checks modes and sizes at every load, this is the helper's own check). An `.mlpackage` or
// `.mlmodel` from the host is refused (`laya.needs_compiled`), because compiling would write a cache into the store;
// a writable file is refused (`laya.not_read_only`). Either way the host's heuristic takes the question. Only the
// developer override `QRTZ_CTRL_LAYA_MODEL` (a path the developer chose at launch) may still be compiled into its
// sibling `.mlmodelc` cache. Nothing here downloads anything or touches the network.
import Foundation

public enum CtrlLayaLoad: Equatable {
    /// Open this `.mlmodelc` as is.
    case asIs(String)
    /// Developer override only: compile this `.mlpackage` / `.mlmodel` into its sibling cache, then open that.
    case compile(String)
    /// A short code; the host keeps the heuristic for the question.
    case refuse(String)

    static let kinds: Set<String> = ["mlmodelc", "mlpackage", "mlmodel"]

    /// The plan for one model path. `fromHost` is true for a path the host sent (the store-linked package), false
    /// for the developer's `QRTZ_CTRL_LAYA_MODEL`.
    public static func plan(path: String, fromHost: Bool, fm: FileManager = .default) -> CtrlLayaLoad {
        let ext = URL(fileURLWithPath: path).pathExtension
        guard !path.isEmpty, kinds.contains(ext), fm.fileExists(atPath: path) else { return .refuse("laya.no_model") }
        guard fromHost else { return ext == "mlmodelc" ? .asIs(path) : .compile(path) }
        guard ext == "mlmodelc" else { return .refuse("laya.needs_compiled") }
        return readOnly(path, fm: fm) ? .asIs(path) : .refuse("laya.not_read_only")
    }

    /// Every regular file under `path` (or `path` itself) has no write bit for anyone, and none is a symlink out.
    /// Mode bits, not access(2), so the answer is the same for root (tests) and the user.
    static func readOnly(_ path: String, fm: FileManager) -> Bool {
        func ok(_ p: String) -> Bool {
            guard let a = try? fm.attributesOfItem(atPath: p), let t = a[.type] as? FileAttributeType else { return false }
            if t == .typeSymbolicLink { return false }
            if t == .typeDirectory { return true }
            guard t == .typeRegular, let mode = (a[.posixPermissions] as? NSNumber)?.uint16Value else { return false }
            return mode & 0o222 == 0
        }
        guard let a = try? fm.attributesOfItem(atPath: path), let t = a[.type] as? FileAttributeType else { return false }
        if t != .typeDirectory { return ok(path) }
        guard let walk = fm.enumerator(atPath: path) else { return false }
        var files = 0
        for case let rel as String in walk {
            guard ok((path as NSString).appendingPathComponent(rel)) else { return false }
            files += 1
        }
        return files > 0
    }
}
