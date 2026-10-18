# ctrl: non-blocking execution (research, slice 1, 2026-10-17/18)

Decision: [ADR 0100](../adr/0100-ctrl-never-takes-over-the-ui.md). Code: `crates/qrtz-ctrl` (ladder, guard, log,
file executor) and `native/qrtz-helper/Sources/QrtzCtrl` (macOS probe and background actions). Open checks for a
real Mac are in [LOG.md](LOG.md).

ctrl's #1 requirement: a task runs while Jared keeps working, and **never blocks or hijacks the screen, the mouse or
the keyboard**. Concretely, after any ctrl action these must be exactly as they were before it:

- the frontmost (active) app,
- the cursor position,
- the focused UI element (where typing goes),
- the front window on screen (nothing raised over what he's looking at).

Every finding below is tagged **[doc]** (Apple documentation or WWDC), **[prior art]** (open-source code or a
shipping product's docs), **[forum]** (developer reports), or **[ours]** (our prototype or tests). Claims that still
need a measurement on a real Mac say so.

## 1. What can act without touching the UI

### 1.1 Plain file and system APIs (no UI at all)

- `FileManager` moves, renames and `trashItem(at:resultingItemURL:)` need no window, no Accessibility and no Apple
  Events. A downloads cleanup is 100% this rung. **[ours]** `CtrlMacGuardTests.testDownloadsCleanupLeavesFrontmostCursorAndFocusAlone`
  moves 20 files and trashes 5 between two guard snapshots and asserts that the frontmost pid, cursor, front window
  and focused element are unchanged (runs in CI's macOS job; see §5).
- The catch is consent, not UI. `~/Downloads`, `~/Desktop` and `~/Documents` are protected by Files and Folders
  privacy, and the first access shows a **modal system prompt** ("qrtz would like to access files in your Downloads
  folder") **[doc]** ([Apple: control access to files and folders](https://support.apple.com/guide/mac-help/control-access-to-files-and-folders-on-mac-mchld5a35146/mac)).
  A modal prompt is a takeover. So ctrl asks for it in the foreground (onboarding or when the task is created) and a
  background run that finds it missing **asks** instead of triggering it.
- Deletion is always a move to the Trash, never an unlink, so every file action has an inverse (spec §13.4).

### 1.2 App scripting (Apple Events) without `activate`

- Scriptable apps (Safari, Chrome, Finder, Mail, Music…) take commands like "close every tab of window 1" over Apple
  Events. A command does not activate the target; only `activate`, `launch`, `open`/`reopen` verbs, or a script that
  includes them, do. cua-driver's no-foreground contract lists exactly these as forbidden for that reason **[prior
  art]** ([cua-driver skill, "The no-foreground contract"](https://github.com/ThinkInAIXYZ/deepchat/blob/dev/plugins/cua/vendor/cua-driver/source/Skills/cua-driver/SKILL.md),
  vendored from [trycua/cua](https://github.com/trycua/cua)).
- ctrl therefore never sends free-form AppleScript. It sends a **fixed set of verb templates** (`ScriptVerb`:
  count/close tabs, count/close windows); the type cannot express `activate`.
- Consent: since macOS 10.14 each (sender, target) pair needs Automation consent (`kTCCServiceAppleEvents`), and the
  first event shows a **modal prompt** **[doc]** ([`NSAppleEventsUsageDescription`](https://developer.apple.com/documentation/bundleresources/information-property-list/nsappleeventsusagedescription)).
  `AEDeterminePermissionToAutomateTarget(…, askUserIfNeeded: false)` checks silently: `errAEEventWouldRequireUserConsent`
  (-1744) means "never asked", `errAEEventNotPermitted` (-1743) means denied **[doc]**
  ([API](https://developer.apple.com/documentation/coreservices/3025784-aedeterminepermissiontoautomatet),
  [-1744](https://developer.apple.com/documentation/coreservices/3025780-anonymous/erraeeventwouldrequireuserconsent?language=objc),
  [Mojave Apple Event APIs](https://www.felix-schwarz.org/blog/2018/08/new-apple-event-apis-in-macos-mojave)).
  Background runs use the silent form only. A hardened-runtime build also needs the
  `com.apple.security.automation.apple-events` entitlement (a Jared decision, see LOG.md).
- Chrome: JavaScript from Apple Events is off by default **[doc]** ([Chromium](https://www.chromium.org/developers/applescript/)).
  ctrl doesn't need it: closing tabs is a plain dictionary command. To verify on the Mac: `sdef` of Safari and Chrome
  (LOG.md).
- Shortcuts (`shortcuts run`) can run user-authored automations, but many actions show UI and their behavior is the
  shortcut author's, not ctrl's. Not a ctrl rung in v1; a later "run this shortcut" task type can treat it as rung 3
  with the same consent rules.

### 1.3 Background Accessibility actions

The AX API lets a trusted process read and act on any app's elements by pid, frontmost or not.

- **Safe in the background:** `AXPress` on in-window controls (buttons, checkboxes, a window's `AXCloseButton`),
  `AXIncrement` / `AXDecrement`, `AXConfirm` / `AXCancel` on sheets, and writing `AXValue`. mac-cua and cua-driver
  both drive background apps this way and rank AX actions and `set_value` as the most reliable, focus-free
  interactions **[prior art]** ([mac-cua](https://github.com/hyprcat/mac-cua) "Reliability Hierarchy"; cua-driver).
  **[ours]** `testBackgroundTextEditOpenAndAXCloseDontTakeOver` closes a background TextEdit document via
  `AXPress` on its close button between two guard snapshots (needs AX trust; see §5 for where it runs).
- **`AXRaise`: refused.** It brings the window above other apps' windows (it "raises them above windows from other
  applications" without activating the app **[forum]** ([Ask Different](https://apple.stackexchange.com/questions/58355/how-can-i-bring-a-window-to-frontmost-without-focusing-it-using-applescript-in-o))).
  It doesn't move keyboard focus, but it covers what Jared is looking at, which is the "screen" half of the
  requirement **[doc]** ([`kAXRaiseAction`](https://developer.apple.com/documentation/applicationservices/kaxraiseaction)).
- **Writing `AXFrontmost`, `AXMain`, `AXFocused`: refused.** These are the AX forms of "activate this app / make this
  the main window / move focus here". Tools use them on purpose to bring an app forward
  ([lingxia-computer-use `focus`](https://docs.rs/crate/lingxia-computer-use/latest/source/src/mac/window_ops.rs)
  sets `AXFrontmost` after `NSRunningApplication.activate`) **[prior art]**. `AXPosition` / `AXSize` /
  `AXFullScreen` move windows on Jared's screen: refused too.
- **Menus: refused.** `AXPress` on `AXPopUpButton`, `AXMenuButton`, `AXComboBox` opens a menu, which starts modal
  mouse tracking on screen. `AXShowMenu` does the same for context menus. Menu-bar items (`AXMenuBarItem`,
  `AXMenuItem`, `AXPick`) are worse in the background: the on-screen menu bar always belongs to the frontmost app,
  and a background app's action items are routinely disabled because their owner isn't key **[prior art]** (cua-driver
  "the AXMenuBar rule"; [Hammerspoon `hs.application`](http://www.hammerspoon.org/docs/0.9.91/hs.application.html)
  notes menu items read as disabled when the app isn't focused). ctrl still **reads** the menu bar to learn an
  app's shortcut table (spec §3); it never presses into it from the background.
- **Secure text fields** (`AXSecureTextField`): never read, written or typed into.
- **"User wins":** if the target app is the one Jared is in and he's been active in the last 2 s, AX and scripting
  actions **defer** rather than compete with him (mac-cua's interruption detection does the same **[prior art]**).

### 1.4 `CGEventPostToPid` (keyboard to a background process)

- `CGEvent.postToPid(_:)` delivers an event to one process's event queue instead of the HID / session stream, so the
  global cursor and the frontmost app's input are untouched **[doc]** ([`postToPid`](<https://developer.apple.com/documentation/coregraphics/cgevent/posttopid(_:)>)).
  mac-cua is built entirely on it ("CGEventPostToPid, never CGEventPost") **[prior art]**.
- **Does the target need to be frontmost?** No for many apps, yes for some. A developer posting to Logic's dialog
  found it ignored the events until Logic was focused **[forum]** ([Apple forums 724835](https://origin-devforums.apple.com/forums/thread/724835)).
  mac-cua documents brief activations for "key-window targeting" and scroll in some apps **[prior art]**. So
  `postToPid` is the **last** allowed rung, and every use is verified by the guard and by `verify` (spec §5).
- **Modifier state.** A `CGEventSource(stateID: .privateState)` keeps its own key state: the events carry exactly the
  flags set on them, aren't merged with keys Jared is holding, and don't change the session's flags **[doc]**
  ([`CGEventSourceStateID.privateState`](https://developer.apple.com/documentation/coregraphics/cgeventsourcestateid/privatestate)).
  mac-cua moved to per-session sources and away from discrete `flagsChanged` events because those "leaked to global
  state"; openwork posts real modifier down/up events because "Chromium and other apps ignore bare flag bits on
  background-posted events" **[prior art]** ([mac-cua](https://github.com/hyprcat/mac-cua),
  [openwork BackgroundInputDispatcher](https://github.com/different-ai/openwork/blob/e8ab197c/packages/handsfree/native/HandsFree/Sources/ComputerUse/BackgroundInputDispatcher.swift)).
  ctrl starts with flags-on-the-event from a private source (no global leak) and measures per app whether that's
  enough. **[ours]** `testPrivateSourceKeyEventsDontTouchTheUsersModifierState` posts ⌘F19 to the test process
  and asserts the session's ⌘ flag didn't change.
- **"Focus here" shortcuts are takeovers even when posted to a pid.** `⌘L` (address bar), `⌘⇧G` (Go to Folder) and
  tab switching (`⌘1…9`, `⌘⇧[ ]`, `⌃⇥`) make the target treat the input as activation intent or visibly flip its
  content **[prior art]** (cua-driver). ctrl refuses them. **This changes spec §2's "open youtube" example:** the
  `cmd-L` option is replaced by "open the URL in the default browser without activation" (rung 2), see LOG.md.
- **Never to the frontmost app.** Keys posted to the app Jared is typing in would interleave with his keystrokes, so
  ctrl refuses `postToPid` to the frontmost app outright.
- **Secure input.** When any app enables Secure Event Input (password fields, some terminals), synthesized keyboard
  events don't reach the secure field and event taps go blind **[doc]** ([TN2150](https://developer.apple.com/library/archive/technotes/tn2150/_index.html)).
  ctrl checks `IsSecureEventInputEnabled()` and **defers** key events while it's on.
- **Permission.** Posting events needs `kTCCServicePostEvent` (checked silently with `CGPreflightPostEventAccess()`
  **[doc]** ([API](<https://developer.apple.com/documentation/coregraphics/cgpreflightposteventaccess()>))). Missing →
  ask in the foreground, never `CGRequestPostEventAccess()` from a background run.
- **Mouse clicks by coordinate** (`postToPid` mouse events) need private window-routing fields (openwork sets
  undocumented fields to target a window) and some apps still warp or activate on hit **[prior art]**. Refused in v1;
  the spec §8 screenshot fallback becomes ask-the-user until a later slice proves a click path on the guard.

### 1.5 Launching and opening without activation

- `NSWorkspace.OpenConfiguration.activates = false` opens a URL or document in an app without activating it
  **[doc]** ([`activates`](https://developer.apple.com/documentation/appkit/nsworkspace/openconfiguration/activates)).
  Since macOS 14, **cooperative app activation** makes `activate` a _request_ that the system can deny "based on
  what the user is doing", explicitly to stop "an app switch while you are in the middle of typing"; only the active
  app can yield activation to another **[doc]** ([WWDC23 "What's new in AppKit", 9:19](https://developer.apple.com/videos/play/wwdc2023/10054/),
  [cooperative activation](https://developer.apple.com/documentation/appkit/passing-control-from-one-app-to-another-with-cooperative-activation)).
  That works in ctrl's favor: a background app that calls `NSApp.activate()` on its own is less likely to win.
- But launches still activate sometimes: mac-cua says "macOS activates apps when they start; mac-cua yields focus
  back immediately", and cua-driver has a `FocusRestoreGuard` that catches a target's own `activate` during
  `application(_:open:)` and puts the frontmost app back **[prior art]**. The `open` CLI always activates (it goes
  through LaunchServices) **[prior art]** (cua-driver).
- ctrl: rung 2 uses `NSWorkspace` with `activates = false` only, never `open`/`tell … to activate`. The guard
  measures what actually happened. A change is a **violation, recorded**; ctrl asks for the previous app to be
  re-activated (`restoreFrontmost`, a request under cooperative activation) and records whether that worked, and the
  (app, rung) is quarantined for the rest of the run. A restore is damage control, not a pass.

## 2. Separate sessions and displays

| Option                                                                                   | What it isolates                                                                                                              | Cost / entitlements                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Distribution                                                                                                                                                                                                                                                                                                            | Verdict                                                                                                                                                     |
| ---------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **CGVirtualDisplay** (private CoreGraphics API, used by BetterDisplay and DeskPad)       | Pixels only. It's another display in the **same** WindowServer session: same cursor, same frontmost app, same keyboard focus. | Private API, can break on any macOS update.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | Not allowed in the Mac App Store; BetterDisplay ships outside it for this reason **[doc/prior art]** ([BetterDisplay FAQ](https://betterdisplay.pro/guide/faq/general/mac-app-store/), [DeskPad](https://github.com/Stengo/DeskPad)). Developer ID notarization doesn't review API use, so it is possible, but fragile. | **No.** Doesn't solve focus or input at all.                                                                                                                |
| **Hidden Spaces** (SkyLight / CGS private APIs, yabai-style)                             | Visibility only, same session.                                                                                                | Private, some operations need SIP partly disabled (yabai docs).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Not App Store; SIP changes are a non-starter for users.                                                                                                                                                                                                                                                                 | **No.**                                                                                                                                                     |
| **Second user session** (fast user switching, or a Screen Sharing login as another user) | A full separate login with its own WindowServer session, cursor and focus.                                                    | A second macOS account; only one console session is on screen at a time.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | n/a                                                                                                                                                                                                                                                                                                                     | **No for ctrl's tasks.** The other account has none of Jared's apps, tabs or files; "close all my tabs" means _his_ session.                                |
| **macOS VM** (Virtualization.framework)                                                  | Everything: its own OS.                                                                                                       | `com.apple.security.virtualization` entitlement (allowed in sandboxed / App Store apps), a restore image download of many GB, several GB of RAM, and the macOS license allows **at most two** additional macOS instances per Mac **[doc]** ([entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.virtualization), [installing macOS in a VM](https://developer.apple.com/documentation/virtualization/installing-macos-on-a-virtual-machine), macOS license: up to two additional instances per Mac, [Apple SLA](https://www.apple.com/legal/sla/), [summary](https://wiki.licenseware.io/catalog/rules/apple-macos-virtualization-up-to-two-additional-instances-per-apple-branded-computer/)). | Feasible.                                                                                                                                                                                                                                                                                                               | **Not v1.** Same problem as a second session (not Jared's apps or data), plus a large consented download. A possible later sandbox for untrusted web tasks. |

Prior art agrees on the split. Microsoft's Agent Workspace gives the agent its own desktop session and account on
Windows; OpenAI's Computer Use runs **in the background on macOS** but "runs on the active desktop and takes over
the foreground" on Windows, where OpenAI suggests a VM if you need to keep working **[doc]**
([ChatGPT Computer Use](https://learn.chatgpt.com/docs/computer-use), [use cases](https://learn.chatgpt.com/use-cases/use-your-computer-with-codex);
`/workspace/ctrl-research/computer-use-report.md` §1A). On macOS the in-session, per-process route is what the
background-capable agents use.

## 3. The strategy (summary of ADR 0100)

A ladder, lowest rung first; each action arrives with the routes the planner found and the ladder takes the lowest
one that's allowed right now:

1. **files**: `FileManager` / `std::fs`.
2. **workspace**: `NSWorkspace` open/launch with `activates = false` (plus the guard's restore on violation).
3. **scripting**: fixed Apple Event verb templates, never `activate`.
4. **ax_action**: background AX actions and value writes, minus raise, menus, focus/frontmost/geometry writes and
   secure fields.
5. **pid_key**: `postToPid` from a private source; never focus-intent shortcuts, never to the frontmost app, never
   during secure input.

Always refused: coordinate clicks, `CGEventPost` to the HID/session tap, any activation (`open -a`,
`NSRunningApplication.activate`, `AXFrontmost`), virtual displays. Missing grants become **ask** (granted in the
foreground), Jared being busy in the target app becomes **defer**, and anything else becomes **refuse** (the task
tells him what it couldn't do without taking over).

Every executed action runs between two **UI guard** snapshots (frontmost pid, cursor, front window, focused-element
id, HID idle time). A difference is a `guard.result` violation in the event log, a restore attempt is recorded as
`guard.restored`, and the (app, rung) is quarantined for the rest of the run (derived from the log by the pure
reducer). A difference while Jared physically touched the mouse or keyboard (HID idle < action time) is attributed
to him and recorded as inconclusive.

## 4. Where the code is and why

- **Rust (`crates/qrtz-ctrl`)**: the ladder, policy tables, guard comparison, event log (hash-chained, pure fold,
  scrub, fork) and the runner effect, plus the file executor. Pure and Linux-testable, like the other `qrtz-*`
  crates; it will own ctrl's state engine (spec §13).
- **Swift (`native/qrtz-helper/Sources/QrtzCtrl`)**: a separate module in the helper package. The helper is the
  process that already holds Accessibility trust for qrtz; ctrl's AX and event code has to run there (TCC grants
  are per app), but in its own module so it can't tangle with echo/nano code. `mac/` holds the macOS-only probe and
  actions; the guard, policy and file ops are Foundation-only and run on Linux CI.
- **Shared fixtures** (`native/qrtz-helper/Fixtures/ctrl/policy.json`, `guard-cases.json`): both sides test their
  tables and guard semantics against the same files, the way the helper protocol uses golden fixtures.
- Not wired into the helper executable or the protocol yet (slice 5); slice 1 is policy, guard and primitives.

## 5. What is proven where

| Claim                                                                                                             | Proven by                                             | Where it runs                                                                                                                                                                                                                                                                                                                                                                                               |
| ----------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ladder order, refusals, ask/defer, quarantine                                                                     | `crates/qrtz-ctrl/tests/ladder.rs`, `tests/runner.rs` | Linux CI (`cargo test`)                                                                                                                                                                                                                                                                                                                                                                                     |
| Guard semantics identical on both sides                                                                           | `guard-cases.json` in Rust and Swift tests            | Linux CI (Rust + Swift), macOS CI (Swift)                                                                                                                                                                                                                                                                                                                                                                   |
| Log determinism, tamper detection, scrub, fork                                                                    | `tests/runner.rs`                                     | Linux CI                                                                                                                                                                                                                                                                                                                                                                                                    |
| File cleanup leaves frontmost, cursor, front window (and focus if trusted) unchanged                              | `CtrlMacGuardTests.testDownloadsCleanup…`             | **macOS CI** (no permission needed)                                                                                                                                                                                                                                                                                                                                                                         |
| Private-source key events don't leak modifiers                                                                    | `CtrlMacGuardTests.testPrivateSourceKeyEvents…`       | macOS CI (posting may be a no-op without `PostEvent`; the leak check still holds)                                                                                                                                                                                                                                                                                                                           |
| Background open + AX close don't take over                                                                        | `CtrlMacGuardTests.testBackgroundTextEdit…`           | Only with AX trust. GitHub's macOS images pre-grant Accessibility to `/bin/bash` and the runner provisioner ([runner-images TCC script](https://github.com/actions/runner-images/blob/main/images/macos/scripts/build/configure-tccdb-macos.sh)), so the test process **may** be trusted there; it skips with a message if not. The first CI run's log line `[ctrl-probe] axTrusted=…` answers it (LOG.md). |
| `postToPid` delivery per app (Safari, Chrome, Finder, TextEdit), `AXPress` on menu items, launch activation rates | not yet                                               | Jared's Mac (LOG.md)                                                                                                                                                                                                                                                                                                                                                                                        |
