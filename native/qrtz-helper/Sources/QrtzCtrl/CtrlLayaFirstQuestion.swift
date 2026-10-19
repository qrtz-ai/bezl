// The first real Laya questions (run 47), Foundation only so it is tested on Linux. The read-only Mac check
// (scripts/ctrl/mac-check) uses it to find the `model.laya` package qrtz installed with ctrl and ask it a few fixed
// questions through the same runtime the helper ships, so the first Core ML load + answer on a Mac is one command.
// It reads only qrtz's own store (`<data dir>/packages/model.laya/<version>/<entry>.mlmodelc`, the layout the qrtz
// package manager links, ADR 0157 §9 / 0169), never downloads, never writes, and asks nothing of the user. Each
// question's expected option is what ctrl's heuristic picks; a different answer is reported, not failed.
import Foundation

public enum CtrlLayaFirstQuestion {
    /// The app's data dir under the user's home (`crates/qrtz-modules/src/install.rs` `default_root`).
    public static let dataDir = "Library/Application Support/ai.qrtz.desktop"
    /// Where the package manager keeps model.laya's versions.
    public static let packageDir = "packages/model.laya"

    public struct Question: Equatable, Sendable {
        public let state: String, question: String, options: [String], expect: Int
    }

    /// Fixed questions whose answer is plain (the fixture's tabs question first: the protocol's own example).
    public static let questions: [Question] = [
        Question(state: "task: close all my tabs", question: "which app should do it?",
                 options: ["safari", "none of these"], expect: 0),
        Question(state: "task: start the screensaver", question: "what does the task ask for?",
                 options: ["open an app", "start the screensaver", "empty the trash", "mute your mac"], expect: 1),
        Question(state: "task: every friday empty the trash", question: "what does the task ask for?",
                 options: ["send a message", "hide or show the dock", "empty the trash"], expect: 2),
    ]

    /// The installed package's `.mlmodelc`: the highest version directory that holds exactly one. `nil` when qrtz
    /// hasn't installed model.laya (ctrl not installed, or its install declined).
    public static func installedModel(root: String, fm: FileManager = .default) -> String? {
        let base = URL(fileURLWithPath: root).appendingPathComponent(packageDir)
        guard let versions = try? fm.contentsOfDirectory(atPath: base.path) else { return nil }
        for v in versions.filter({ version($0) != nil }).sorted(by: { version($1)!.lexicographicallyPrecedes(version($0)!) }) {
            let dir = base.appendingPathComponent(v)
            let models = ((try? fm.contentsOfDirectory(atPath: dir.path)) ?? []).filter { $0.hasSuffix(".mlmodelc") }
            if models.count == 1 { return dir.appendingPathComponent(models[0]).path }
        }
        return nil
    }

    /// `1.0.0` → [1, 0, 0]; anything else (a staging dir, a dotfile) → nil.
    static func version(_ s: String) -> [Int]? {
        let parts = s.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3 else { return nil }
        let n = parts.compactMap { Int($0) }
        return n.count == 3 ? n : nil
    }

    /// One answer's report line: the top option, whether it is the expected one, the probabilities (3 decimals) and
    /// the time. An error code instead when the runtime refused (the host would keep the heuristic for it).
    public static func report(_ q: Question, probabilities p: [Double]?, error: String?, ms: Int) -> [String: Any] {
        var r: [String: Any] = ["question": q.question, "state": q.state, "ms": ms]
        if let e = error { r["error"] = e; return r }
        guard let p, p.count == q.options.count, let top = p.indices.max(by: { p[$0] < p[$1] }) else {
            r["error"] = "laya.bad_model"
            return r
        }
        r["top"] = q.options[top]
        r["as_expected"] = top == q.expect
        r["p"] = p.map { ($0 * 1000).rounded() / 1000 }
        return r
    }
}
