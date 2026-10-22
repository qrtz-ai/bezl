// qrtz-bubble-lab: the echo dictation bubble (the pill) on its own, for iterating on its look and motion.
// Hold fn: the real NativePill shows (NotchSurface's show animation, PillWave's level animation fed by fake levels).
// Let go: it hides with the real hide animation. Nothing else: no mic, no recording, no model, no text insertion,
// no host. Every view, animation and the fn monitor (MacHotkeys / HotkeyFilter) are the shipping ones, so a change
// here changes the real bubble.
//
//   cd native/qrtz-helper && swift run qrtz-bubble-lab [--idle] [--processing]
//     --idle        feed level 0 (the quiet wave) instead of a fake voice
//     --processing  on release, show the processing swell for 1.2 s before hiding
//
// Permissions (granted to the app you run it from, e.g. Terminal or iTerm):
//   Input Monitoring  needed: the fn listen tap sees nothing without it.
//   Accessibility     optional: lets it swallow fn so macOS's globe action (emoji, input source) doesn't also run.
#if os(macOS)
import AppKit
import ApplicationServices
import CoreGraphics
import QrtzEcho
import QrtzEchoMac
import QrtzMacShared
import QrtzShared

logTag = "qrtz-bubble-lab"
let args = CommandLine.arguments
let idle = args.contains("--idle")
let processing = args.contains("--processing")

func say(_ s: String) { print("[qrtz-bubble-lab] \(s)"); fflush(stdout) }

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let pill = NativePill()

// Fake levels while held: a voice-ish envelope (syllables plus a little noise), or 0 with --idle.
var held = false
var t0 = Date()
let ticker = Timer(timeInterval: 1.0 / 60.0, repeats: true) { _ in
    guard held else { return }
    if idle { pill.level(0); return }
    let t = Date().timeIntervalSince(t0)
    let syllable = max(0, sin(t * 2 * .pi * 3.1)) * (0.55 + 0.45 * sin(t * 2 * .pi * 0.37))
    pill.level(min(1, max(0, 0.35 * syllable + Double.random(in: 0...0.04))))
}
RunLoop.main.add(ticker, forMode: .common)

var releaseGen = 0
func edge(_ a: HotkeyAction) {
    switch a {
    case .down:
        held = true; t0 = Date(); releaseGen += 1
        pill.show(.listening)
    case .up:
        held = false
        pill.level(0)
        if processing {
            releaseGen += 1
            let g = releaseGen
            pill.show(.processing)
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { if g == releaseGen { pill.show(.hidden) } }
        } else {
            pill.show(.hidden)
        }
    }
}

if !CGPreflightListenEventAccess() {
    say("Input Monitoring is OFF for this terminal: fn presses won't be seen.")
    say("  System Settings → Privacy & Security → Input Monitoring → enable your terminal app, then rerun.")
    _ = CGRequestListenEventAccess()
}
if !AXIsProcessTrusted() {
    say("Accessibility is off (optional): fn still works, but macOS's globe action may also run on release.")
}

let hotkeys = MacHotkeys()
// The taps call back on the main run loop.
if !hotkeys.start(key: "fn", consumeFn: true, onChange: { a in DispatchQueue.main.async { edge(a) } }) {
    say("could not create the fn event tap (grant Input Monitoring); retrying every 2 s.")
} else {
    say("ready: hold fn to open the bubble, let go to close it. ctrl-c to quit.")
}
signal(SIGINT) { _ in exit(0) }
app.run()
#else
print("qrtz-bubble-lab: macOS only")
#endif
