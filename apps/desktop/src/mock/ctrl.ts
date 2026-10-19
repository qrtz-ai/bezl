// The mocked backend's ctrl (e2e and `dev:mock`): the same store semantics as crates/qrtz-ctrl `store.rs`, in
// memory. Ids come from one append counter (`task-0`, `run-1`); a run records `task.started`, `intent.pending`,
// `obs.apps`, then canned decision-layer chains of the same shape the real layers produce (ADR 0106): "tabs"
// → close tabs, "downloads" / "folder" → clean up a folder, anything else asks which intent. `ctrl:run` folds the
// first `at` events. The real layers, persistence and replay are covered by `cargo test -p qrtz-ctrl`.
// The mock acts as if the ctrl helper were connected (ADR 0108): a close-tabs plan executes in the background
// (one guarded action, verified) and finishes `done`; a folder cleanup stays `planned` (file routes are slice 7).
// The real executor against a fake helper is covered by `crates/qrtz-ctrl/tests/exec.rs`.
// "quit" → quit textedit, irreversible: the run stops at the confirm gate (ADR 0111) with two windows (one unsaved)
// until `ctrl:confirm` approves (acts once), remembers (a standing approval: the next run doesn't ask) or denies.
// The real gate is covered by `crates/qrtz-ctrl/tests/gate.rs`.
import {
  AppError,
  type CtrlConfirm,
  type CtrlDecision,
  type CtrlModelState,
  type CtrlOption,
  type CtrlPlan,
  type CtrlRun,
  type CtrlRunState,
  type CtrlRunView,
  type CtrlTask,
  type Handlers,
} from '@qrtz/core';

/**
 * ADR 0158: the dev mock's stand-in for "has the user made the qrtz Set Focus shortcut": `?mockFocusShortcut=1`
 * (made), `=denied` (Automation denied), absent (not made yet).
 */
const focusShortcut = (): 'missing' | 'ready' | 'denied' => {
  const v = new URLSearchParams(window.location.search).get(
    'mockFocusShortcut',
  );
  return v === 'denied' ? 'denied' : v ? 'ready' : 'missing';
};

type Row = CtrlRunView['events'][number];
interface Rec {
  row: Row;
  decision?: CtrlDecision;
  plan?: CtrlPlan;
  answer?: { decisionId: string; choice: string | null };
  finished?: 'cancelled' | 'done' | 'asked-out';
  confirm?: CtrlConfirm;
  answerGate?: NonNullable<CtrlConfirm['answer']>;
  reverified?: true;
  action?: { id: string; status: 'executing' | 'done' };
  verified?: { actionId: string; outcome: 'passed' };
  held?:
    | 'files-not-yet'
    | 'folder-not-yet'
    | 'no-route'
    | 'focus-no-shortcut'
    | 'automation-denied';
  /** A folder cleanup's batch (ADR 0117): its files, and which of them moved since (a "renamed" task). */
  batch?: { files: string[]; moved: string[] };
  undone?: number;
}

const opt = (id: string, label: string, p: number): CtrlOption => ({
  id,
  label,
  p,
  irreversible: false,
  refusedInBackground: false,
});
const NONE = opt('none', 'none of these', 0.01);
const act = (
  node: CtrlDecision['node'],
  options: CtrlOption[],
): Omit<CtrlDecision, 'id'> => ({
  node,
  outcome: 'act',
  answered: false,
  chosen: options[0]!.id,
  chosenLabel: options[0]!.label,
  p: options[0]!.p,
  model: 'qrtz-heuristic 1',
  options: [...options, NONE],
});

const INTENTS = {
  close_tabs: opt('intent.close_tabs', 'close browser tabs', 0.07),
  clean_folder: opt('intent.clean_folder', 'clean up a folder', 0.07),
  trash_old: opt('intent.clean_folder', 'clean up a folder', 0.07),
  tidy_documents: opt('intent.clean_folder', 'clean up a folder', 0.07),
  tidy_desktop: opt('intent.clean_folder', 'clean up a folder', 0.07),
  open_calendar: opt('intent.open_app', 'open an app', 0.07),
  close_windows: opt('intent.close_windows', 'close windows', 0.07),
  empty_trash: {
    ...opt('intent.empty_trash', 'empty the trash', 0.07),
    irreversible: true,
  },
  quit_app: {
    ...opt('intent.quit_app', 'quit an app', 0.07),
    irreversible: true,
  },
  quit_unused: {
    ...opt('intent.quit_unused', "quit apps you haven't used today", 0.07),
    irreversible: true,
  },
  hide_others: opt('intent.hide_others', 'hide your other apps', 0.07),
  hide_app: opt('intent.hide_app', 'hide an app', 0.07),
  show_app: opt('intent.show_app', 'show a hidden app', 0.07),
  unminimize_app: opt(
    'intent.unminimize_app',
    'bring back minimized windows',
    0.07,
  ),
  minimize_app: opt('intent.minimize_app', 'minimize windows', 0.07),
  open_folder: opt('intent.open_folder', 'open a folder', 0.07),
  focus_mode: opt(
    'intent.focus_mode',
    'turn focus or do not disturb on or off',
    0.07,
  ),
  focus_off: opt(
    'intent.focus_mode',
    'turn focus or do not disturb on or off',
    0.07,
  ),
  clear_clipboard: opt('intent.clear_clipboard', 'clear your clipboard', 0.07),
  eject_all: opt('intent.eject_disks', 'eject your disks', 0.07),
  eject_one: opt('intent.eject_disks', 'eject your disks', 0.07),
  mute: opt('intent.mute', 'mute your mac', 0.07),
  unmute: opt('intent.unmute', 'unmute your mac', 0.07),
  set_volume: opt('intent.set_volume', 'set the volume', 0.07),
  volume_up: opt('intent.change_volume', 'turn the volume up or down', 0.07),
  volume_down: opt('intent.change_volume', 'turn the volume up or down', 0.07),
  dark_mode: opt('intent.dark_mode', 'switch dark mode', 0.07),
  light_mode: opt('intent.dark_mode', 'switch dark mode', 0.07),
  dark_toggle: opt('intent.dark_mode', 'switch dark mode', 0.07),
  dock_hide: opt('intent.dock_autohide', 'hide or show the dock', 0.07),
  dock_show: opt('intent.dock_autohide', 'hide or show the dock', 0.07),
  dock_edge_left: opt('intent.dock_edge', 'move the dock to an edge', 0.07),
  dock_edge_right: opt('intent.dock_edge', 'move the dock to an edge', 0.07),
  dock_edge_bottom: opt('intent.dock_edge', 'move the dock to an edge', 0.07),
  dock_smaller: opt(
    'intent.dock_size',
    'make the dock smaller or bigger',
    0.07,
  ),
  dock_bigger: opt('intent.dock_size', 'make the dock smaller or bigger', 0.07),
  dock_magnify_on: opt(
    'intent.dock_magnification',
    'turn dock magnification on or off',
    0.07,
  ),
  dock_magnify_off: opt(
    'intent.dock_magnification',
    'turn dock magnification on or off',
    0.07,
  ),
  menu_bar_hide: opt(
    'intent.menu_bar_autohide',
    'hide or show the menu bar',
    0.07,
  ),
  menu_bar_show: opt(
    'intent.menu_bar_autohide',
    'hide or show the menu bar',
    0.07,
  ),
  screensaver: opt('intent.screensaver', 'start or stop the screensaver', 0.07),
  screensaver_stop: opt(
    'intent.screensaver',
    'start or stop the screensaver',
    0.07,
  ),
  display_sleep: opt('intent.display_sleep', 'turn off your display', 0.07),
  system_sleep: opt('intent.system_sleep', 'put your mac to sleep', 0.07),
  wifi_on: opt('intent.wifi_power', 'turn wi-fi on or off', 0.07),
  wifi_off: opt('intent.wifi_power', 'turn wi-fi on or off', 0.07),
  audio_built_in: opt('intent.audio_output', 'switch the sound output', 0.07),
  audio_bluetooth: opt('intent.audio_output', 'switch the sound output', 0.07),
  audio_usb: opt('intent.audio_output', 'switch the sound output', 0.07),
  audio_display: opt('intent.audio_output', 'switch the sound output', 0.07),
  keep_awake: opt('intent.keep_awake', 'keep your mac awake for a while', 0.07),
  keep_screen_on: opt(
    'intent.keep_awake',
    'keep your mac awake for a while',
    0.07,
  ),
  allow_sleep: opt('intent.allow_sleep', 'let your mac sleep again', 0.07),
  open_email: opt('intent.open_app', 'open an app', 0.07),
};
type Intent =
  | 'close_tabs'
  | 'clean_folder'
  | 'trash_old'
  | 'tidy_documents'
  | 'tidy_desktop'
  | 'open_calendar'
  | 'empty_trash'
  | 'quit_app'
  | 'quit_unused'
  | 'hide_others'
  | 'hide_app'
  | 'show_app'
  | 'unminimize_app'
  | 'minimize_app'
  | 'open_folder'
  | 'focus_mode'
  | 'focus_off'
  | 'clear_clipboard'
  | 'eject_all'
  | 'eject_one'
  | 'mute'
  | 'unmute'
  | 'set_volume'
  | 'volume_up'
  | 'volume_down'
  | 'dark_mode'
  | 'light_mode'
  | 'dark_toggle'
  | 'dock_hide'
  | 'dock_show'
  | 'dock_edge_left'
  | 'dock_edge_right'
  | 'dock_edge_bottom'
  | 'dock_smaller'
  | 'dock_bigger'
  | 'dock_magnify_on'
  | 'dock_magnify_off'
  | 'menu_bar_hide'
  | 'menu_bar_show'
  | 'screensaver'
  | 'screensaver_stop'
  | 'display_sleep'
  | 'system_sleep'
  | 'wifi_on'
  | 'wifi_off'
  | 'audio_built_in'
  | 'audio_bluetooth'
  | 'audio_usb'
  | 'audio_display'
  | 'keep_awake'
  | 'keep_screen_on'
  | 'allow_sleep'
  | 'open_email';
/** What an unclear task is asked (the three most likely intents). */
const ASKED = [INTENTS.close_tabs, INTENTS.clean_folder, INTENTS.close_windows];

/** After the intent: the rest of the chain, all acting. */
const REST: Record<Intent, Omit<CtrlDecision, 'id'>[]> = {
  close_tabs: [
    act('choose_app', [
      opt('app.default_browser', 'your default browser', 0.88),
      opt('app.com.google.Chrome', 'google chrome', 0.04),
    ]),
    act('choose_target', [
      opt('scope.all_tabs', 'all tabs', 0.99),
      opt('scope.front_tab', 'front tab', 0.0),
    ]),
    act('choose_move', [
      opt('route.scripting.close_all_tabs', 'close every tab by script', 0.94),
      opt('shortcut.cmd+w', 'cmd-w close tab', 0.02),
    ]),
  ],
  quit_app: [
    act('choose_app', [
      opt('app.com.apple.TextEdit', 'textedit', 0.9),
      opt('app.com.apple.Notes', 'notes', 0.03),
    ]),
    act('choose_move', [
      // ADR 0148: one named app quits through the workspace (terminate, no key event, no activation).
      {
        ...opt(
          'route.workspace.quit_app',
          'quit it without bringing it forward',
          0.95,
        ),
        irreversible: true,
      },
    ]),
  ],
  // ADR 0130: the apps not used today (the helper's activation record), quit behind the card; no app question.
  quit_unused: [
    act('choose_move', [
      {
        ...opt(
          'route.workspace.quit_unused',
          'quit them without bringing anything forward',
          0.97,
        ),
        irreversible: true,
      },
    ]),
  ],
  // ADR 0134: every running app but the frontmost one and qrtz, hidden; reversible, so no card and no app question.
  hide_others: [
    act('choose_move', [
      opt(
        'route.workspace.hide_others',
        "hide them, the app you're in stays",
        0.97,
      ),
    ]),
  ],
  // ADR 0149: one named app hidden through the workspace (no key event, no activation); reversible, no card.
  hide_app: [
    act('choose_app', [
      opt('app.com.tinyspeck.slackmacgap', 'slack', 0.9),
      opt('app.com.apple.Notes', 'notes', 0.03),
    ]),
    act('choose_move', [
      opt('route.workspace.hide_app', 'hide it, nothing comes forward', 0.95),
    ]),
  ],
  // ADR 0152: one named hidden app unhidden through the workspace (nothing activated); reversible, no card.
  show_app: [
    act('choose_app', [
      opt('app.com.apple.TextEdit', 'textedit', 0.9),
      opt('app.com.apple.Notes', 'notes', 0.03),
    ]),
    act('choose_move', [
      opt(
        'route.workspace.show_app',
        "show its windows, the app you're in stays in front",
        0.95,
      ),
    ]),
  ],
  // ADR 0166: one of your folders in a finder window (activates = false); reversible, no card. The folder is decided
  // from the task in finishChain (a closed set, never a path).
  open_folder: [
    act('choose_move', [
      opt(
        'route.workspace.open_folder',
        'show it in a finder window, nothing comes forward',
        0.95,
      ),
    ]),
  ],
  // ADR 0137: the appearance, through system events (faceless); reversible, no card.
  dark_mode: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dark_mode_on', 'switch to dark mode', 0.9),
      opt('route.scripting.dark_mode_off', 'switch to light mode', 0.05),
    ]),
  ],
  light_mode: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dark_mode_off', 'switch to light mode', 0.9),
      opt('route.scripting.dark_mode_on', 'switch to dark mode', 0.05),
    ]),
  ],
  // ADR 0145: "toggle dark mode" takes the other appearance from the one read just before; no card.
  dark_toggle: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dark_mode_toggle', 'flip the appearance', 0.9),
      opt('route.scripting.dark_mode_on', 'switch to dark mode', 0.05),
      opt('route.scripting.dark_mode_off', 'switch to light mode', 0.05),
    ]),
  ],
  // ADR 0173: the dock's auto-hide, through system events (faceless) like the appearance; reversible, no card.
  dock_hide: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_autohide_on',
        'hide the dock until the pointer reaches it',
        0.9,
      ),
      opt('route.scripting.dock_autohide_off', 'always show the dock', 0.05),
    ]),
  ],
  dock_show: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dock_autohide_off', 'always show the dock', 0.9),
      opt(
        'route.scripting.dock_autohide_on',
        'hide the dock until the pointer reaches it',
        0.05,
      ),
    ]),
  ],
  // ADR 0195: the dock's screen edge, through system events; reversible, no card.
  dock_edge_left: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dock_edge_left', 'put the dock on the left', 0.9),
      opt('route.scripting.dock_edge_right', 'put the dock on the right', 0.05),
      opt(
        'route.scripting.dock_edge_bottom',
        'put the dock on the bottom',
        0.05,
      ),
    ]),
  ],
  dock_edge_right: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt('route.scripting.dock_edge_right', 'put the dock on the right', 0.9),
      opt('route.scripting.dock_edge_left', 'put the dock on the left', 0.05),
      opt(
        'route.scripting.dock_edge_bottom',
        'put the dock on the bottom',
        0.05,
      ),
    ]),
  ],
  // ADR 0200: the dock's size one step, through system events like its edge; one move from the words, no card.
  dock_smaller: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_size_smaller',
        'make the dock a step smaller',
        0.9,
      ),
    ]),
  ],
  dock_bigger: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_size_bigger',
        'make the dock a step bigger',
        0.9,
      ),
    ]),
  ],
  // ADR 0202: the dock's magnification on or off, through system events like its size; one move from the words, no card.
  dock_magnify_on: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_magnification_on',
        "magnify the dock's icons under the pointer",
        0.9,
      ),
    ]),
  ],
  dock_magnify_off: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_magnification_off',
        "stop magnifying the dock's icons",
        0.9,
      ),
    ]),
  ],
  dock_edge_bottom: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.dock_edge_bottom',
        'put the dock on the bottom',
        0.9,
      ),
      opt('route.scripting.dock_edge_left', 'put the dock on the left', 0.05),
      opt('route.scripting.dock_edge_right', 'put the dock on the right', 0.05),
    ]),
  ],
  // ADR 0197: the menu bar's auto-hide, through system events like the dock's; one move from the words, no card.
  menu_bar_hide: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.menu_bar_autohide_on',
        'hide the menu bar until the pointer reaches it',
        0.9,
      ),
    ]),
  ],
  menu_bar_show: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.menu_bar_autohide_off',
        'always show the menu bar, full screen as set',
        0.9,
      ),
    ]),
  ],
  // ADR 0178: start the current screen saver through system events; reversible by the user, no card.
  screensaver: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.screensaver',
        'start the current screen saver',
        0.95,
      ),
    ]),
  ],
  // ADR 0178 amendment 1: stop it the same way; a password prompt after it stays (nothing unlocked), no card.
  screensaver_stop: [
    act('choose_app', [
      opt('app.com.apple.systemevents', 'systemevents', 0.95),
    ]),
    act('choose_move', [
      opt(
        'route.scripting.screensaver_stop',
        'stop the screen saver, nothing unlocked',
        0.95,
      ),
    ]),
  ],
  // ADR 0180: the displays sleep in the helper's own process (the mac stays awake); no app question, no card.
  display_sleep: [
    act('choose_move', [
      opt(
        'route.system.display_sleep',
        'put the displays to sleep, the mac stays awake',
        0.97,
      ),
    ]),
  ],
  // ADR 0187: held until the mac probe shows an AXMinimized write raises nothing; no app question, asked out.
  unminimize_app: [
    act('choose_move', [
      opt(
        'route.windows.unminimize_hold',
        'leave them in the dock, not checked on a mac yet',
        0.97,
      ),
    ]),
  ],
  minimize_app: [
    act('choose_move', [
      opt(
        'route.windows.minimize_hold',
        'leave the windows as they are, not checked on a mac yet',
        0.97,
      ),
    ]),
  ],
  // ADR 0183: the whole mac sleeps in the helper's own process; no app question, no card.
  system_sleep: [
    act('choose_move', [
      opt(
        'route.system.system_sleep',
        'put the whole mac to sleep, nothing closed',
        0.97,
      ),
    ]),
  ],
  // ADR 0191: the wi-fi radio's power in the helper's own process, one direction; no app question, no card.
  wifi_on: [
    act('choose_move', [
      opt(
        'route.system.wifi_on',
        'turn the wi-fi radio on, no network picked',
        0.97,
      ),
    ]),
  ],
  wifi_off: [
    act('choose_move', [
      opt(
        'route.system.wifi_off',
        'turn the wi-fi radio off, nothing else changed',
        0.97,
      ),
    ]),
  ],
  // ADR 0198: the one connected output device of the kind the words name becomes the sound output, picked in the
  // helper's own process; no app question, no card.
  audio_built_in: [
    act('choose_move', [
      opt(
        'route.system.audio_output_built_in',
        "play sound through the mac's own speakers",
        0.97,
      ),
    ]),
  ],
  audio_bluetooth: [
    act('choose_move', [
      opt(
        'route.system.audio_output_bluetooth',
        'play sound through the bluetooth device, like airpods',
        0.97,
      ),
    ]),
  ],
  audio_usb: [
    act('choose_move', [
      opt(
        'route.system.audio_output_usb',
        'play sound through the usb device',
        0.97,
      ),
    ]),
  ],
  audio_display: [
    act('choose_move', [
      opt(
        'route.system.audio_output_display',
        "play sound through the display's speakers",
        0.97,
      ),
    ]),
  ],
  // ADR 0194: one power assertion of the helper's own with a timeout (choose_target `minutes.N`, added in
  // finishChain); nothing on screen, no card.
  keep_awake: [
    act('choose_move', [
      opt(
        'route.system.keep_awake',
        'keep the mac from sleeping, the screen may still sleep',
        0.97,
      ),
    ]),
  ],
  keep_screen_on: [
    act('choose_move', [
      opt(
        'route.system.keep_awake_display',
        'keep the mac and its screen from sleeping',
        0.97,
      ),
    ]),
  ],
  allow_sleep: [
    act('choose_move', [
      opt(
        'route.system.allow_sleep',
        'stop keeping it awake, nothing put to sleep',
        0.97,
      ),
    ]),
  ],
  // ADR 0138: "my email" is mail, already running, so it's already open (nothing comes forward).
  open_email: [
    act('choose_app', [opt('app.com.apple.mail', 'mail', 0.95)]),
    act('choose_move', [
      opt('route.workspace.launch_hidden', 'start it hidden', 0.95),
    ]),
  ],
  // ADR 0140: the level is the task's own number (choose_target, added in finishChain); reversible, no card.
  set_volume: [
    act('choose_move', [
      opt(
        'route.system.set_volume',
        'set the sound output to that level',
        0.97,
      ),
    ]),
  ],
  // ADR 0143: a step from the level read just before (choose_target `step.N`, added in finishChain); no card.
  volume_up: [
    act('choose_move', [
      opt('route.system.volume_up', 'turn the sound output up a step', 0.97),
    ]),
  ],
  volume_down: [
    act('choose_move', [
      opt(
        'route.system.volume_down',
        'turn the sound output down a step',
        0.97,
      ),
    ]),
  ],
  // ADR 0139: the reverse of mute; reversible, no card, no app question.
  unmute: [
    act('choose_move', [
      opt('route.system.unmute', 'turn the sound output back on', 0.97),
    ]),
  ],
  // ADR 0158: the user's "qrtz Set Focus" shortcut through Shortcuts Events, with a mode from a closed set.
  focus_mode: [
    act('choose_move', [
      opt('route.system.focus.do_not_disturb', 'turn on do not disturb', 0.97),
    ]),
  ],
  focus_off: [
    act('choose_move', [opt('route.system.focus.off', 'turn focus off', 0.97)]),
  ],
  // ADR 0168: the clipboard, emptied in the helper's own process (nothing read from it); no card, no app question.
  clear_clipboard: [
    act('choose_move', [
      opt(
        'route.system.clear_clipboard',
        'empty the clipboard, nothing read from it',
        0.97,
      ),
    ]),
  ],
  // ADR 0176: the ejectable local volumes, unmounted in the helper's own process (never forced); no card, no app
  // question. One move only: a singular task never backtracks into ejecting every disk.
  eject_all: [
    act('choose_move', [
      opt(
        'route.system.eject_all',
        'eject every external disk, none forced',
        0.97,
      ),
    ]),
  ],
  eject_one: [
    act('choose_move', [
      opt(
        'route.system.eject_one',
        'eject the one external disk, never forced',
        0.97,
      ),
    ]),
  ],
  // ADR 0135: the sound output, muted in the helper's own process; reversible, no card, no app question.
  mute: [
    act('choose_move', [
      opt('route.system.mute', 'mute the sound output', 0.97),
    ]),
  ],
  clean_folder: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.downloads', 'downloads', 0.98),
      opt('folder.desktop', 'desktop', 0.01),
    ]),
    act('choose_move', [
      opt('route.files.tidy', 'sort by kind, nothing deleted', 0.89),
      opt('route.files.sort_by_kind', 'sort files into folders by kind', 0.03),
    ]),
  ],
  // ADR 0125: the Documents folder has no routes yet; its one move is a hold that says so.
  tidy_documents: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.documents', 'documents', 0.98),
      opt('folder.downloads', 'downloads', 0.01),
    ]),
    act('choose_move', [
      opt(
        'route.files.documents_hold',
        'leave documents as they are, not supported yet',
        0.97,
      ),
    ]),
  ],
  // ADR 0124 / 0126: only macOS's own screenshots and screen recordings move, into the folder screenshots are
  // saved to (desktop/screenshots unless the screenshot app saves them somewhere else); nothing is deleted.
  tidy_desktop: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.desktop', 'desktop', 0.98),
      opt('folder.downloads', 'downloads', 0.01),
    ]),
    act('choose_move', [
      opt(
        'route.files.desktop_screenshots',
        'move screenshots into a folder, nothing deleted',
        0.95,
      ),
      {
        ...opt(
          'route.files.trash_screenshots',
          'move screenshots to the trash',
          0.02,
        ),
        irreversible: true,
      },
    ]),
  ],
  // ADR 0128: "my calendar" is calendar.app; a running one is already open (nothing launched, nothing hidden).
  open_calendar: [
    act('choose_app', [
      opt('app.com.apple.iCal', 'calendar', 0.95),
      opt('app.name.notion-calendar', 'notion calendar', 0.03),
    ]),
    act('choose_move', [
      opt('route.workspace.launch_hidden', 'start it hidden', 0.97),
    ]),
  ],
  // ADR 0122: emptying the Trash deletes through the file executor (never the finder), always behind the card.
  empty_trash: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_move', [
      {
        ...opt('route.files.empty_trash', 'empty the trash', 0.9),
        irreversible: true,
      },
      {
        ...opt(
          'shortcut.cmd+shift+delete',
          'cmd-shift-delete empty trash',
          0.05,
        ),
        irreversible: true,
      },
    ]),
  ],
  trash_old: [
    act('choose_app', [
      opt('app.files', 'files (no app)', 0.93),
      opt('app.com.apple.finder', 'finder', 0.05),
    ]),
    act('choose_target', [
      opt('folder.downloads', 'downloads', 0.98),
      opt('folder.desktop', 'desktop', 0.01),
    ]),
    act('choose_move', [
      {
        ...opt(
          'route.files.trash_old',
          'move old installers to the trash',
          0.9,
        ),
        irreversible: true,
      },
      opt('route.files.tidy', 'sort by kind, nothing deleted', 0.05),
    ]),
  ],
};

/** ADR 0140: the level a volume task names (the number after "to" / "volume"), 0–100. */
const levelOf = (text: string): number | null => {
  // ADR 0143: "all the way up / down" is the end of the range.
  const end = /\ball the way (up|down)\b/i.exec(text);
  if (end) return end[1]?.toLowerCase() === 'up' ? 100 : 0;
  const m = /\b(?:to|volume)\s+(\d{1,3})\b/i.exec(text);
  const n = m ? Number(m[1]) : NaN;
  return n >= 0 && n <= 100 ? n : null;
};

/** ADR 0143: up / down from the words, or null (none or both). */
const directionOf = (text: string): 'volume_up' | 'volume_down' | null => {
  const up = /\b(up|louder|raise|increase|higher|boost)\b/i.test(text);
  const down = /\b(down|quieter|softer|lower|decrease|reduce)\b/i.test(text);
  return up === down ? null : up ? 'volume_up' : 'volume_down';
};

/** ADR 0143: the step of a relative change: "by N" / "up N" / "down N", "a bit" is 5, else 10. */
// ADR 0166: the folder "open my …" shows, from the closed set.
const folderOf = (
  text: string,
): 'downloads' | 'desktop' | 'documents' | 'pictures' | 'music' | 'movies' =>
  /\b(documents|docs)\b/i.test(text)
    ? 'documents'
    : /\bdesktop\b/i.test(text)
      ? 'desktop'
      : /\b(pictures|pics)\b/i.test(text)
        ? 'pictures'
        : /\bmusic\b/i.test(text)
          ? 'music'
          : /\b(movies|videos)\b/i.test(text)
            ? 'movies'
            : 'downloads';
// Run 40 (ADR 0166 amendment 1): pictures / movies need "my" or "folder"; music only with "folder" (else the music app).
const namesFolder = (text: string): boolean =>
  /\b(downloads?|desktop|documents|docs)\b/i.test(text) ||
  (/\b(pictures|pics|movies|videos)\b/i.test(text) &&
    /\b(my|folder)\b/i.test(text)) ||
  (/\bmusic\b/i.test(text) && /\bfolder\b/i.test(text));
const opensFolder = (text: string): boolean =>
  (/\b(open|browse|view)\b/i.test(text) ||
    (/\b(show|go)\b/i.test(text) && /\b(my|folder)\b/i.test(text))) &&
  namesFolder(text) &&
  !/\b(clean|tidy|organi[sz]e|sort|declutter|clear|delete|trash|remove|move)\b/i.test(
    text,
  );

const stepOf = (text: string): number => {
  const m = /\b(?:by|up|down)\s+(\d{1,3})\b/i.exec(text);
  const n = m ? Number(m[1]) : NaN;
  if (n >= 1 && n <= 100) return n;
  return /\b(bit|little|slightly|tad)\b/i.test(text) ? 5 : 10;
};

const relativeOf = (text: string): Intent | null =>
  levelOf(text) === null &&
  (/\b(volume|sound|audio|louder|quieter|softer)\b/i.test(text) ||
    /\bturn\b.*\bit\b/i.test(text))
    ? directionOf(text)
    : null;

// ADR 0168: a clipboard word (or "what i copied") plus a clearing word.
const clearsClipboard = (text: string): boolean =>
  (/\b(clipboard|pasteboard)\b/i.test(text) ||
    /\b(what|whatever|everything|i)\b.*\bcopied\b/i.test(text)) &&
  /\b(clear|empty|wipe|erase|delete|remove|reset|flush|purge)\b/i.test(text);

// ADR 0176: an eject word; every disk with a plural or an all-word ("every" only right before a disk word).
const ejectOf = (text: string): Intent | null =>
  /\b(eject|ejects|ejecting|unmount|unmounts|unmounting)\b/i.test(text)
    ? /\b(all|everything|both|disks|drives|volumes|sticks|cards|images|dmgs|ssds|externals|devices)\b/i.test(
        text,
      ) ||
      /\bevery\s+(disk|drive|volume|stick|card|image|dmg|ssd|usb|sd|external|device)\b/i.test(
        text,
      )
      ? 'eject_all'
      : 'eject_one'
    : null;

// ADR 0195: "dock" plus left / right / bottom, no hide / show / autohide (those stay dock_autohide).
// ADR 0200: the dock plus a size or magnification word, whatever else is said (never its auto-hide or edge then).
const dockSizeWording = (text: string): boolean =>
  /\bdock\b/i.test(text) &&
  /\b(smaller|tinier|littler|shrinks?|shrinking|reduce|decrease|bigger|larger|enlarge|grows?|increase|expand|small|big|large|tiny|huge|smallest|biggest|largest|tiniest|sizes?|sized|magnif\w*|zoom)\b/i.test(
    text,
  );
// ADR 0200: "make the dock smaller" / "make the dock bigger": one step, one way; not magnification, an absolute size,
// text, windows, a toggle, a negation, a mixed dock task or a number.
const dockSizeOf = (text: string): Intent | null => {
  if (!dockSizeWording(text)) return null;
  if (
    /\b(magnif\w*|zoom|smallest|biggest|largest|minimum|maximum|min|max|default|normal|original|reset|text|font|labels?|windows?|toggle|flip|don't|dont|never|not|stop|keep|hide|hides|hiding|hidden|auto-?hide|auto-?hiding|automatically|show|shows|shown|showing|visible|unhide|always|left|right|bottom)\b|\d|menu\s*-?bar/i.test(
      text,
    )
  )
    return null;
  const smaller =
    /\b(smaller|tinier|littler|shrinks?|shrinking|reduce|decrease)\b/i.test(
      text,
    );
  const bigger = /\b(bigger|larger|enlarge|grows?|increase|expand)\b/i.test(
    text,
  );
  if (smaller && !bigger) return 'dock_smaller';
  if (bigger && !smaller) return 'dock_bigger';
  return null;
};

// ADR 0202: "turn on dock magnification" / "turn off magnification on the dock": one way; not its size, a level, a
// toggle, a negation, zoom, a mixed dock task or a number. An "on" before the / my / dock / hover / a day is a place,
// not a switch.
const dockMagnifyOf = (text: string): Intent | null => {
  if (
    !/\bdock\b/i.test(text) ||
    !/\bmagnif(y|ies|ied|ying|ication|ier)\b/i.test(text)
  )
    return null;
  if (
    /\b(toggle|flip|don't|dont|never|not|no|sizes?|sized|amount|level|more|less|much|small|big|large|tiny|huge|smallest|biggest|largest|tiniest|minimum|maximum|min|max|default|normal|original|reset|zoom|text|font|labels?|windows?|smaller|tinier|littler|shrinks?|shrinking|reduce|decrease|bigger|larger|enlarge|grows?|increase|expand|hide|hides|hiding|hidden|auto-?hide|auto-?hiding|automatically|show|shows|shown|showing|visible|unhide|left|right|bottom)\b|\d|menu\s*-?bar/i.test(
      text,
    )
  )
    return null;
  const on =
    /\b(turn|turned|switch|switched|put)\s+on\b/i.test(text) ||
    /\bon\b(?!\s+(the|my|your|this|that|its|dock|hover|mouseover|mouse|pointer|(mon|tues|wednes|thurs|fri|satur|sun)days?|weekdays?|weekends?)\b)/i.test(
      text,
    ) ||
    /\b(enable|enabled|start|activate|magnify|magnifies)\b/i.test(text);
  const off = /\b(off|disable|disabled|stop|deactivate)\b/i.test(text);
  if (on && !off) return 'dock_magnify_on';
  if (off && !on) return 'dock_magnify_off';
  return null;
};

const dockEdgeOf = (text: string): Intent | null => {
  if (!/\bdock\b/i.test(text) || /\b(toggle|flip)\b/i.test(text)) return null;
  if (dockSizeWording(text)) return null;
  if (
    /\b(hide|hides|hiding|hidden|auto-?hide|auto-?hiding|automatically|show|shows|shown|showing|visible|unhide|always)\b/i.test(
      text,
    )
  )
    return null;
  const left = /\bleft\b/i.test(text);
  const right = /\bright\b/i.test(text);
  const bottom = /\bbottom\b/i.test(text);
  if (left && !right && !bottom) return 'dock_edge_left';
  if (right && !left && !bottom) return 'dock_edge_right';
  if (bottom && !left && !right) return 'dock_edge_bottom';
  return null;
};

// ADR 0197: the menu bar plus a hide / show / on-off word; no dock, no menu-bar items, full-screen / desktop-only
// modes, look or toggle. Off when it says show, stop, don't, off, never (the dock's words).
const menuBarNamed = (text: string): boolean =>
  /\b(menubar|menu-bar)\b/i.test(text) || /\bmenu\s+bar\b/i.test(text);
const menuBarOf = (text: string): Intent | null => {
  if (!menuBarNamed(text) || /\bdock\b/i.test(text)) return null;
  if (
    /\b(toggle|flip|full|fullscreen|full-screen|desktop|icons?|items?|extras?|clock|date|battery|wifi|wi-fi|bluetooth|sound|volume|spotlight|siri|control|center|centre|background|transparent|transparency|tint|colou?r|size|spacing)\b/i.test(
      text,
    )
  )
    return null;
  const hide =
    /\b(hide|hides|hiding|hidden|auto-?hide|auto-?hiding|automatically)\b/i.test(
      text,
    );
  const show = /\b(show|shows|shown|showing|visible|unhide|always)\b/i.test(
    text,
  );
  if (!hide && !show && !/\b(turn|enable|disable|on|off|keep)\b/i.test(text))
    return null;
  if (/\b(stop|don't|dont|never|off|disable|no|not|unhide)\b/i.test(text))
    return 'menu_bar_show';
  return hide && (!show || /\b(automatically|auto-?hide)\b/i.test(text))
    ? 'menu_bar_hide'
    : 'menu_bar_show';
};

// ADR 0173: "dock" plus a hide / show / on-off word (not a toggle); off when it says show, stop, don't, off, never.
// Not an edge move (ADR 0195).
const dockOf = (text: string): Intent | null => {
  if (!/\bdock\b/i.test(text) || /\b(toggle|flip)\b/i.test(text)) return null;
  if (dockEdgeOf(text) !== null) return null;
  // ADR 0197: "hide the dock and the menu bar" is two tasks, neither here.
  if (menuBarNamed(text)) return null;
  // ADR 0200: nor is a dock task with a size or magnification word.
  if (dockSizeWording(text)) return null;
  const hide =
    /\b(hide|hides|hiding|hidden|auto-?hide|auto-?hiding|automatically)\b/i.test(
      text,
    );
  const show = /\b(show|shows|shown|showing|visible|unhide|always)\b/i.test(
    text,
  );
  if (!hide && !show && !/\b(turn|enable|disable|on|off|keep)\b/i.test(text))
    return null;
  if (/\b(stop|don't|dont|never|off|disable|no|not|unhide)\b/i.test(text))
    return 'dock_show';
  return hide && (!show || /\b(automatically|auto-?hide)\b/i.test(text))
    ? 'dock_hide'
    : 'dock_show';
};

// ADR 0178: screensaver / screen saver plus a start word, or (amendment 1) a stop word; never its setting
// (off / disable / never), a negation or lock.
const screensaverOf = (text: string): Intent | null => {
  const named =
    /\bscreensaver\b/i.test(text) ||
    (/\bscreen\b/i.test(text) && /\bsaver\b/i.test(text));
  if (!named) return null;
  if (
    /\b(disable|disables|off|never|don't|dont|not|no|lock|locks|locking|locked|unlock|unlocks|password)\b/i.test(
      text,
    )
  )
    return null;
  const stop =
    /\b(stop|stops|stopping|quit|quits|exit|exits|end|ends|dismiss|dismisses|close|closes)\b/i.test(
      text,
    );
  if (stop)
    return /\b(start|starts|starting|begin|begins|launch|launches|activate|on)\b/i.test(
      text,
    )
      ? null
      : 'screensaver_stop';
  if (
    !/\b(start|starts|starting|run|runs|running|launch|launches|begin|begins|activate|enable|on|turn)\b/i.test(
      text,
    )
  )
    return null;
  return 'screensaver';
};

// ADR 0180: a display word plus sleep / off; never the screen saver, lock, screen time, sharing, recording,
// screenshots, brightness, notifications or keeping it awake.
const displaySleepOf = (text: string): Intent | null => {
  if (!/\b(display|displays|screen|screens|monitor|monitors)\b/i.test(text))
    return null;
  if (!/\b(sleep|sleeps|asleep|off)\b/i.test(text)) return null;
  if (
    /\b(saver|savers|screensavers?|lock|locks|locked|locking|sharing|share|recording|record|recorder|screenshots?|shots?|mirror|mirroring|brightness|bright|brighter|dim|dimmer|reader|voiceover|notifications?|wake|wakes|awake|keep|never|prevent|stop|don't|dont|not|caffeinate)\b/i.test(
      text,
    ) ||
    /\bscreen\s+time\b/i.test(text)
  )
    return null;
  return 'display_sleep';
};

// ADR 0183: a mac word plus sleep; never the display alone, keeping it awake, waking it, its sleep settings, a
// sleep timer, lock, restart, shut down or log out.
const systemSleepOf = (text: string): Intent | null => {
  if (
    !/\b(mac|macs|macbook|macbooks|imac|computer|computers|laptop|laptops|machine|system)\b/i.test(
      text,
    )
  )
    return null;
  if (!/\b(sleep|asleep)\b/i.test(text)) return null;
  if (
    /\b(display|displays|screen|screens|monitor|monitors|screensaver|saver|wake|wakes|waking|awake|keep|never|prevent|stop|don't|dont|not|caffeinate|settings|setting|preferences|timer|lock|locked|restart|reboot|shutdown|shut|log|logout)\b/i.test(
      text,
    )
  )
    return null;
  return 'system_sleep';
};

// ADR 0187: un-minimize ("unminimize slack", "restore slack windows", "show my minimized windows", "bring slack back
// from the dock") or minimize ("minimize slack"); never the dock's minimize settings.
const windowsOf = (text: string): Intent | null => {
  if (
    /\b(effect|genie|scale|animation|settings?|preferences?|icon|double-click)\b/i.test(
      text,
    )
  )
    return null;
  const minimized = /\b(minimi[sz](e|es|ed|ing))\b/i.test(text);
  if (
    /\b(un-?minimi[sz]e[ds]?|deminimi[sz]e|deminiaturize)\b/i.test(text) ||
    (minimized &&
      /\b(restores?|bring|back|show|get|open|reopen|unhide|return)\b/i.test(
        text,
      )) ||
    (/\brestores?\b/i.test(text) && /\bwindows?\b/i.test(text)) ||
    (/\bdock\b/i.test(text) &&
      /\b(bring|get|take|pull)\b/i.test(text) &&
      /\b(back|out|from)\b/i.test(text))
  )
    return 'unminimize_app';
  return minimized ? 'minimize_app' : null;
};

// ADR 0191: wi-fi named plus exactly one direction ("on" not as "on my mac" / "on weekdays"); never a network by name
// or password, joining / leaving one, the hotspot, wi-fi calling, its settings, a toggle or restart, another radio or
// a negation.
const wifiOf = (text: string): Intent | null => {
  if (!/\b(wifi|wi-fi|wi fi|wifis)\b/i.test(text)) return null;
  if (
    /\b(password|passwords|network|networks|ssid|name|join|joins|connect|connects|connected|connecting|reconnect|disconnect|disconnects|forget|scan|hotspot|tether|tethering|share|sharing|calling|calls|settings|setting|preferences|toggle|flip|restart|reset|reboot|cycle|bluetooth|airdrop|airplane|ethernet|vpn|signal|speed|router|don't|dont|never|not|keep|stays|stay|leave)\b/i.test(
      text,
    )
  )
    return null;
  const off = /\b(off|disable|disabled|deactivate)\b/i.test(text);
  const on =
    /\b(enable|enabled|activate|reenable|re-enable)\b/i.test(text) ||
    /\b(turn|turns|switch|switches|flick)\s+on\b/i.test(text) ||
    /\S\s+on\b(?!\s+(my|the|this|that|your|our|a|an|weekdays?|weekends?|(mon|tues|wednes|thurs|fri|satur|sun)days?)\b)/i.test(
      text,
    );
  if (on === off) return null;
  return on ? 'wifi_on' : 'wifi_off';
};

// ADR 0198: a switch verb, where the sound comes out (a sound word or an audio device) and exactly one kind: the mac's
// own speakers, bluetooth / airpods, usb, a display. "from x to y" counts only y. Never the input side, the volume or
// mute, pairing or connecting, airplay / homepod / apple tv, several outputs, alert sounds, settings, a toggle or a
// negation.
const audioOf = (text: string): Intent | null => {
  if (
    !/\b(switch|switches|change|set|use|move|send|route|play|put|make|select|pick|choose|swap|redirect|go)\b/i.test(
      text,
    )
  )
    return null;
  if (
    !/\b(sound|audio|output|outputs|speakers?|playback|airpods?|headphones?|headsets?|earbuds|earphones)\b/i.test(
      text,
    )
  )
    return null;
  if (
    /\b(input|inputs|mics?|microphones?|record|recording|dictation|volume|louder|quieter|softer|mute|unmute|silence|off|connect|connects|connected|connecting|reconnect|disconnect|pair|unpair|forget|battery|noise|transparency|cancellation|spatial|airplay|homepods?|both|all|alerts?|effects|notifications?|settings?|preferences|toggle|flip|don't|dont|never|not|stop)\b/i.test(
      text,
    ) ||
    /\bapple\s+tv\b/i.test(text)
  )
    return null;
  const to = /\bfrom\b.*\bto\b(.*)$/i.exec(text);
  const target = to ? (to[1] ?? '') : text;
  const kinds: Intent[] = [];
  if (
    /\b(built-in|builtin|internal)\b/i.test(target) ||
    /(?<!\b(on|of|in)\s+(my|the|this|your|our)?\s*)\b(mac|mac's|macbook|macbook's|laptop|laptop's|computer|computer's|imac|imac's)(?![\w'])/i.test(
      target,
    )
  )
    kinds.push('audio_built_in');
  if (/\b(airpods?|bluetooth)\b/i.test(target)) kinds.push('audio_bluetooth');
  if (/\busb\b/i.test(target)) kinds.push('audio_usb');
  if (
    /\b(display|display's|displays|monitor|monitor's|monitors|tv|tv's|television|hdmi|displayport)\b/i.test(
      target,
    )
  )
    kinds.push('audio_display');
  return kinds.length === 1 ? (kinds[0] ?? null) : null;
};

// ADR 0194: keep the mac (a screen word: and its screen) awake, or let it sleep again; never without an end, a
// condition, the sleep settings, waking or locking.
const awakeOf = (text: string): Intent | null => {
  if (
    /\b(until|till|forever|always|indefinitely|never|while|when|if|settings|wake|lock|screensaver|wifi|wi-fi)\b/i.test(
      text,
    )
  )
    return null;
  const sleep = /\b(sleep|sleeps|sleeping|asleep)\b/i.test(text);
  const allow =
    (sleep &&
      /\b(let|allow)\b/i.test(text) &&
      !/\b(don't|dont|not|prevent)\b/i.test(text)) ||
    (/\b(stop|cancel|end)\b/i.test(text) &&
      /\b(keeping|caffeinate|awake)\b/i.test(text) &&
      !/\bfrom\b/i.test(text));
  if (allow) return 'allow_sleep';
  const keep = /\b(keep|stay)\b/i.test(text);
  const screen = /\b(screen|display|monitor)\b/i.test(text);
  const awake =
    (keep && /\bawake\b/i.test(text)) ||
    /\bcaffeinate\b/i.test(text) ||
    (sleep && /\b(prevent|don't|dont|not|from)\b/i.test(text)) ||
    (keep && screen && /\bon\b/i.test(text));
  if (!awake) return null;
  return screen ? 'keep_screen_on' : 'keep_awake';
};

/** ADR 0194: the minutes the task says (1–720), an hour when it says none, null when it's out of range. */
const minutesOf = (text: string): number | null => {
  if (/\bhalf an hour\b/i.test(text)) return 30;
  const m = /\b(\d{1,4}|an?|one)\s+(hours?|hrs?|minutes?|mins?)\b/i.exec(text);
  if (!m) return 60;
  const count = /^\d+$/.test(m[1] ?? '') ? Number(m[1]) : 1;
  const n = /^h/i.test(m[2] ?? '') ? count * 60 : count;
  return n >= 1 && n <= 720 ? n : null;
};

const minutesLabel = (n: number): string =>
  n === 60
    ? 'for an hour'
    : n % 60 === 0
      ? `for ${n / 60} hours`
      : `for ${n} minutes`;

const intentOf = (text: string): Intent | null =>
  dockMagnifyOf(text) ??
  dockSizeOf(text) ??
  windowsOf(text) ??
  awakeOf(text) ??
  wifiOf(text) ??
  audioOf(text) ??
  ejectOf(text) ??
  displaySleepOf(text) ??
  systemSleepOf(text) ??
  (clearsClipboard(text)
    ? 'clear_clipboard'
    : screensaverOf(text) !== null
      ? screensaverOf(text)
      : menuBarOf(text) !== null
        ? menuBarOf(text)
        : dockEdgeOf(text) !== null
          ? dockEdgeOf(text)
          : dockOf(text) !== null
            ? dockOf(text)
            : relativeOf(text) !== null
              ? relativeOf(text)
              : /\b(volume|sound)\b/i.test(text) && levelOf(text) !== null
                ? 'set_volume'
                : /\b(disturb|dnd)\b/i.test(text) ||
                    (/\bfocus\b/i.test(text) &&
                      /\b(on|off|turn|enable|disable|start|stop|end|mode|set|toggle)\b/i.test(
                        text,
                      ))
                  ? /\b(off|disable|stop|end)\b/i.test(text)
                    ? 'focus_off'
                    : 'focus_mode'
                  : /\b(unmute|unsilence)\b/i.test(text)
                    ? 'unmute'
                    : /\b(dark|light)\s+mode\b/i.test(text)
                      ? /\b(toggle|flip)\b/i.test(text)
                        ? 'dark_toggle'
                        : /\b(light|off|disable)\b/i.test(text)
                          ? 'light_mode'
                          : 'dark_mode'
                      : /\b(open|check)\b/i.test(text) &&
                          /\b(email|emails|inbox)\b/i.test(text)
                        ? 'open_email'
                        : /\b(mute|silence)\b/i.test(text)
                          ? 'mute'
                          : /\bhide\b/i.test(text) &&
                              /\b(apps|applications|other|others|everything|all)\b/i.test(
                                text,
                              )
                            ? 'hide_others'
                            : /\bhide\b/i.test(text)
                              ? 'hide_app'
                              : opensFolder(text)
                                ? 'open_folder'
                                : /\bunhide\b/i.test(text) ||
                                    (/\bshow\b/i.test(text) &&
                                      !/\b(me|my)\b/i.test(text))
                                  ? 'show_app'
                                  : /\b(quit|close)\b/i.test(text) &&
                                      /\bapps?\b/i.test(text) &&
                                      /\b(unused|used|using|idle)\b/i.test(text)
                                    ? 'quit_unused'
                                    : /\bquit\b/i.test(text)
                                      ? 'quit_app'
                                      : /\bcalendar\b/i.test(text)
                                        ? 'open_calendar'
                                        : /\bempty\b/i.test(text) &&
                                            /\b(trash|bin)\b/i.test(text)
                                          ? 'empty_trash'
                                          : /\btabs?\b/i.test(text)
                                            ? 'close_tabs'
                                            : /\bdocuments\b/i.test(text)
                                              ? 'tidy_documents'
                                              : /\b(desktop|screenshots?)\b/i.test(
                                                    text,
                                                  )
                                                ? 'tidy_desktop'
                                                : /download|folder/i.test(text)
                                                  ? /\b(delete|trash|old)\b/i.test(
                                                      text,
                                                    )
                                                    ? 'trash_old'
                                                    : 'clean_folder'
                                                  : null);

const PREVIEW: CtrlConfirm['items'] = [
  { key: '3f2a9c1d0b7e#0', kind: 'window', label: 'notes.txt', unsaved: false },
  { key: '8c1e44a0d2f9#0', kind: 'window', label: 'draft.txt', unsaved: true },
];

/** ADR 0119: tabs held because the browser blocks ctrl's page script (a task that says "blocked"): can't tell. */
const BLIND: CtrlConfirm['items'] = [
  { key: '5d1c0a9e8f7b#0', kind: 'tab', label: 'inbox', unsaved: null },
  { key: 'a7b8c9d0e1f2#0', kind: 'tab', label: 'expense form', unsaved: null },
];

/** ADR 0148: "quit notes", the frontmost app in the mock: the plan, then held (never the app the user is in). */
const QUIT_FRONT = [
  act('choose_app', [opt('app.com.apple.Notes', 'notes', 0.9)]),
  act('choose_move', [
    {
      ...opt(
        'route.workspace.quit_app',
        'quit it without bringing it forward',
        0.95,
      ),
      irreversible: true,
    },
  ]),
];

/** ADR 0149: "hide notes", the frontmost app in the mock: the plan, then held. */
const HIDE_FRONT = [
  act('choose_app', [opt('app.com.apple.Notes', 'notes', 0.9)]),
  act('choose_move', [
    opt('route.workspace.hide_app', 'hide it, nothing comes forward', 0.95),
  ]),
];

/** ADR 0130: the running apps a quit_unused run would quit (not used today; frontmost, finder and qrtz never). */
const UNUSED: CtrlConfirm['items'] = [
  { key: '0a1b2c3d4e5f#0', kind: 'app', label: 'slack', unsaved: null },
  { key: '5f4e3d2c1b0a#0', kind: 'app', label: 'textedit', unsaved: true },
];

/** ADR 0122: what an empty_trash run would delete (the Finder's own files and anything just trashed are left). */
const TRASH: CtrlConfirm['items'] = [
  { key: 'f0c1d2e3f4a5', kind: 'file', label: 'old folder', unsaved: false },
  { key: 'f5a4f3e2d1c0', kind: 'file', label: 'old.dmg', unsaved: false },
];

/** Slice 7 (ADR 0113): the old installers a trash_old run would send to the Trash. */
const FILES: CtrlConfirm['items'] = [
  { key: 'f1a2b3c4d5e6', kind: 'file', label: 'ancient.pkg', unsaved: false },
  { key: 'f6e5d4c3b2a1', kind: 'file', label: 'oldapp.dmg', unsaved: false },
];

export function ctrlHandlers(now = () => Date.now()): Partial<Handlers> {
  let seq = 0;
  const tasks: CtrlTask[] = [];
  const runs = new Map<string, { run: CtrlRun; recs: Rec[] }>();
  const task = (id: string) => {
    const t = tasks.find((x) => x.id === id);
    if (!t) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
    return t;
  };
  const stateAt = (recs: Rec[], at: number): CtrlRunState => {
    const decisions: CtrlDecision[] = [];
    let plan: CtrlPlan | null = null;
    let ask: CtrlRunState['ask'] = null;
    let pending = false;
    let finished: Rec['finished'] | null = null;
    const actions = new Map<string, 'executing' | 'done'>();
    const verified: NonNullable<Rec['verified']>[] = [];
    let held: Rec['held'] | null = null;
    let confirm: CtrlConfirm | null = null;
    let batch: Rec['batch'] | null = null;
    let undone: number | null = null;
    for (const r of recs.slice(0, at)) {
      if (r.batch) batch = r.batch;
      if (r.undone !== undefined) undone = r.undone;
      if (r.confirm) confirm = r.confirm;
      if (r.answerGate && confirm)
        confirm = { ...confirm, answer: r.answerGate };
      if (r.reverified && confirm) confirm = { ...confirm, reverified: true };
      if (r.action && confirm) confirm = { ...confirm, used: true };
      if (r.held) held = r.held;
      if (r.action) actions.set(r.action.id, r.action.status);
      if (r.verified) verified.push(r.verified);
      if (r.row.kind === 'intent.pending') pending = true;
      if (r.decision) {
        decisions.push(r.decision);
        if (r.decision.outcome === 'ask')
          ask = {
            decisionId: r.decision.id,
            node: r.decision.node,
            options: r.decision.options.filter((o) => o.id !== 'none'),
          };
      }
      if (r.answer) {
        ask = null;
        const d = decisions.find((x) => x.id === r.answer!.decisionId);
        const c = d?.options.find((o) => o.id === r.answer!.choice);
        if (d && c) {
          const i = decisions.indexOf(d);
          decisions[i] = {
            ...d,
            answered: true,
            chosen: c.id,
            chosenLabel: c.label,
          };
        }
      }
      if (r.plan) {
        plan = r.plan;
        pending = false;
      }
      if (r.finished) finished = r.finished;
    }
    const status = finished
      ? finished
      : ask
        ? 'asking'
        : confirm && !confirm.answer
          ? 'needs-confirm'
          : pending
            ? 'pending-intent'
            : plan && !actions.size
              ? 'planned'
              : 'running';
    return {
      actions: [...actions].map(([id, st]) => ({ id, status: st })),
      verified,
      held,
      violations: 0,
      inconclusive: 0,
      quarantined: [],
      status,
      decisions,
      plan,
      ask,
      confirm,
      undo:
        batch && finished === 'done'
          ? {
              actionId: 'a0',
              files: batch.files.length,
              ready:
                undone !== null || at < recs.length
                  ? 0
                  : batch.files.length - batch.moved.length,
              cantUndo: batch.moved.map((f) => ({
                key: `f-${f}`,
                label: f,
                why: 'moved' as const,
              })),
              done: undone === null ? null : { undone },
            }
          : null,
    };
  };
  const push = (
    r: { run: CtrlRun; recs: Rec[] },
    kind: string,
    extra: Omit<Rec, 'row'> = {},
    node: string | null = null,
  ) => {
    r.recs.push({
      row: { seq: r.recs.length, kind, at: now(), actionId: null, node },
      ...extra,
    });
  };
  const decide = (
    r: { run: CtrlRun; recs: Rec[] },
    d: Omit<CtrlDecision, 'id'>,
  ) => {
    const n = r.recs.filter((x) => x.decision).length;
    push(r, 'decision.made', { decision: { ...d, id: `d${n}` } }, d.node);
  };
  const execute = (r: { run: CtrlRun; recs: Rec[] }) => {
    // The executor (ADR 0108): one guarded action, then its verify step.
    const a = (kind: string, extra: Omit<Rec, 'row'> = {}) =>
      r.recs.push({
        row: {
          seq: r.recs.length,
          kind,
          at: now(),
          actionId: 'a0',
          node: null,
        },
        ...extra,
      });
    a('action.requested', { action: { id: 'a0', status: 'executing' } });
    a('action.decided');
    a('guard.snapshot');
    a('guard.snapshot');
    a('action.executed', { action: { id: 'a0', status: 'done' } });
    a('guard.result');
    a('verify.result', { verified: { actionId: 'a0', outcome: 'passed' } });
    push(r, 'task.finished', { finished: 'done' });
  };
  const finishChain = (
    r: { run: CtrlRun; recs: Rec[] },
    intent: Intent,
    text = '',
  ) => {
    const level = intent === 'set_volume' ? levelOf(text) : null;
    if (level !== null)
      decide(
        r,
        act('choose_target', [opt(`level.${level}`, `${level}%`, 0.96)]),
      );
    if (intent === 'keep_awake' || intent === 'keep_screen_on') {
      const n = minutesOf(text) ?? 60;
      decide(
        r,
        act('choose_target', [opt(`minutes.${n}`, minutesLabel(n), 0.96)]),
      );
    }
    if (intent === 'open_folder') {
      const f = folderOf(text);
      decide(r, act('choose_target', [opt(`folder.${f}`, f, 0.97)]));
    }
    if (intent === 'volume_up' || intent === 'volume_down') {
      const n = stepOf(text);
      decide(r, act('choose_target', [opt(`step.${n}`, `by ${n}`, 0.96)]));
    }
    // ADR 0148: notes is the app the user is in (the mock's frontmost), so "quit notes" is held before the card.
    const quitFront = intent === 'quit_app' && /\bnotes\b/i.test(text);
    // ADR 0149: likewise "hide notes" is held (hiding the app you're in would bring another app forward).
    const hideFront = intent === 'hide_app' && /\bnotes\b/i.test(text);
    for (const d of quitFront
      ? QUIT_FRONT
      : hideFront
        ? HIDE_FRONT
        : REST[intent])
      decide(r, d);
    const s = stateAt(r.recs, r.recs.length);
    push(r, 'plan.produced', {
      plan: {
        irreversible:
          intent === 'quit_app' ||
          intent === 'quit_unused' ||
          intent === 'trash_old' ||
          intent === 'empty_trash',
        steps: s.decisions.map((d) => ({
          node: d.node,
          option: d.chosen,
          label: d.chosenLabel,
        })),
      },
    });
    if (quitFront || hideFront) {
      push(r, 'exec.held', { held: 'no-route' });
      push(r, 'task.finished', { finished: 'asked-out' });
      return;
    }
    if (
      (intent === 'focus_mode' || intent === 'focus_off') &&
      focusShortcut() !== 'ready'
    ) {
      // ADR 0158: the shortcut isn't made yet (or Automation is denied): the run stops and says what to do; no
      // fallback is tried.
      push(r, 'exec.held', {
        held:
          focusShortcut() === 'denied'
            ? 'automation-denied'
            : 'focus-no-shortcut',
      });
      push(r, 'task.finished', { finished: 'asked-out' });
      return;
    }
    if (intent === 'unminimize_app' || intent === 'minimize_app') {
      // ADR 0187: held before anything is read or sent, until the mac probe; the run ends asked out.
      push(r, 'exec.held', { held: 'no-route' });
      push(r, 'task.finished', { finished: 'asked-out' });
      return;
    }
    if (intent === 'tidy_documents') {
      // ADR 0125: held before any folder is read; the run ends and says why.
      push(r, 'exec.held', { held: 'folder-not-yet' });
      push(r, 'task.finished', { finished: 'asked-out' });
      return;
    }
    if (
      intent === 'close_tabs' &&
      /\bblocked\b/i.test(task(r.run.taskId).text)
    ) {
      // ADR 0114: the tabs ctrl can't read are held behind the card (the plain ones would have closed already).
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${r.recs.filter((x) => x.confirm).length}`,
          action: 'close browser tabs',
          app: 'safari',
          move: 'close tabs with unsaved input',
          items: BLIND,
          complete: true,
          rules: ['choose_move:route.scripting.close_unsaved_tabs'],
          answer: null,
          reverified: null,
          used: false,
        },
      });
      return;
    }
    if (intent === 'quit_unused') {
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${r.recs.filter((x) => x.confirm).length}`,
          action: "quit apps you haven't used today",
          app: null,
          move: 'quit them without bringing anything forward',
          items: UNUSED,
          complete: true,
          rules: [
            'choose_intent:intent.quit_unused',
            'choose_move:route.workspace.quit_unused',
          ],
          answer: null,
          reverified: null,
          used: false,
        },
      });
      return;
    }
    if (
      intent === 'quit_app' ||
      intent === 'trash_old' ||
      intent === 'empty_trash'
    ) {
      // The confirm gate (ADR 0111): a standing approval covers it, or the run waits for the user.
      const g = r.recs.filter((x) => x.confirm).length;
      const quit = intent === 'quit_app';
      const empty = intent === 'empty_trash';
      push(r, 'confirm.requested', {
        confirm: {
          gateId: `g${g}`,
          action: quit
            ? 'quit an app'
            : empty
              ? 'empty the trash'
              : 'clean up a folder',
          app: quit ? 'textedit' : 'files (no app)',
          move: quit
            ? 'quit'
            : empty
              ? 'empty the trash'
              : 'move old installers to the trash',
          items: quit ? PREVIEW : empty ? TRASH : FILES,
          complete: true,
          rules: quit
            ? ['choose_intent:intent.quit_app', 'choose_move:shortcut.cmd+q']
            : empty
              ? [
                  'choose_intent:intent.empty_trash',
                  'choose_move:route.files.empty_trash',
                ]
              : ['choose_move:route.files.trash_old'],
          answer: null,
          reverified: null,
          used: false,
        },
      });
      if (task(r.run.taskId).approvals?.length) {
        push(r, 'confirm.resolved', { answerGate: 'standing' });
        push(r, 'confirm.reverified', { reverified: true });
        execute(r);
      }
      return;
    }
    // Slice 7 (ADR 0113): a folder cleanup is one batch of file moves, guarded and verified like any action.
    execute(r);
    if (intent === 'tidy_desktop') {
      r.recs.find((x) => x.row.kind === 'action.requested')!.batch = {
        files: [
          'Screenshot 2026-10-12 at 09.14.02.png',
          'Screen Recording 2026-10-15 at 16.20.11.mov',
        ],
        moved: [],
      };
      return;
    }
    if (intent !== 'clean_folder') return;
    const moved = /\brenamed\b/i.test(task(r.run.taskId).text)
      ? ['photo.png']
      : [];
    r.recs.find((x) => x.row.kind === 'action.requested')!.batch = {
      files: ['photo.png', 'taxes 2025.pdf', 'site.zip'],
      moved,
    };
  };
  const sync = (r: { run: CtrlRun; recs: Rec[] }) => {
    r.run.events = r.recs.length;
    r.run.status = stateAt(r.recs, r.recs.length).status;
  };
  const view = (r: { run: CtrlRun; recs: Rec[] }, at?: number) => {
    const n = Math.min(at ?? r.recs.length, r.recs.length);
    return {
      run: r.run,
      events: r.recs.map((x) => x.row),
      at: n,
      state: stateAt(r.recs, n),
    };
  };
  let model: CtrlModelState = {
    consented: false,
    state: 'off',
    // ADR 0129: laya-coreml @ 7b8d7a2, e8 L512 bucket + tokenizer.json.
    sizeBytes: 483_924_446,
    bytesOnDisk: 0,
    scorer: 'heuristic',
    error: null,
  };
  const get = (runId: string) => {
    const r = runs.get(runId);
    if (!r) throw new AppError('E_NOT_FOUND', 'that task or run is gone');
    return r;
  };
  return {
    'ctrl:tasks': () => [...tasks].reverse(),
    'ctrl:create-task': ({ text, schedule }) => {
      const t: CtrlTask = {
        id: `task-${seq++}`,
        text: text.trim(),
        schedule,
        paused: false,
        createdAt: now(),
        runs: 0,
        lastRunAt: null,
        // Mock has no ticker; ASAP shows as due now, cadences as null (real nextRunAt is server-side).
        nextRunAt: schedule.kind === 'once' ? (schedule.at ?? now()) : null,
        approvals: [],
      };
      tasks.push(t);
      return t;
    },
    'ctrl:set-paused': ({ id, paused }) => {
      const t = task(id);
      if (t.paused !== paused) seq++;
      t.paused = paused;
      if (paused) t.nextRunAt = null;
      return t;
    },
    'ctrl:delete-task': ({ id }) => {
      tasks.splice(tasks.indexOf(task(id)), 1);
      seq++;
      return { ok: true as const };
    },
    'ctrl:run-now': ({ id }) => {
      const t = task(id);
      const at = now();
      const r = {
        run: {
          id: `run-${seq++}`,
          taskId: id,
          at,
          status: 'pending-intent',
          events: 0,
          violations: 0,
        } as CtrlRun,
        recs: [] as Rec[],
      };
      push(r, 'task.started');
      push(r, 'intent.pending');
      push(r, 'obs.apps');
      const intent = intentOf(t.text);
      if (intent) {
        decide(r, act('choose_intent', [{ ...INTENTS[intent], p: 0.93 }]));
        finishChain(r, intent, t.text);
      } else {
        decide(r, {
          ...act('choose_intent', ASKED),
          outcome: 'ask',
          chosen: 'none',
          chosenLabel: 'none of these',
          p: 0.2,
        });
      }
      sync(r);
      runs.set(r.run.id, r);
      t.runs += 1;
      t.lastRunAt = at;
      if (t.schedule.kind === 'once') t.nextRunAt = null;
      return r.run;
    },
    'ctrl:runs': ({ taskId }) =>
      [...runs.values()]
        .filter((r) => r.run.taskId === task(taskId).id)
        .map((r) => r.run)
        .reverse(),
    'ctrl:run': ({ runId, at }) => view(get(runId), at),
    'ctrl:answer': ({ runId, decisionId, optionId }) => {
      const r = get(runId);
      const s = stateAt(r.recs, r.recs.length);
      if (s.ask?.decisionId !== decisionId)
        throw new AppError(
          'E_VALIDATION',
          'that question was already answered',
        );
      if (optionId !== null && !s.ask.options.some((o) => o.id === optionId))
        throw new AppError('E_VALIDATION', 'pick one of the options');
      push(r, 'decision.answered_by_user', {
        answer: { decisionId, choice: optionId },
      });
      if (optionId === null)
        push(r, 'task.finished', { finished: 'cancelled' });
      else {
        const intent = optionId.replace('intent.', '') as Intent;
        if (intent in REST) finishChain(r, intent);
      }
      sync(r);
      return view(r);
    },
    'ctrl:confirm': ({ runId, gateId, answer }) => {
      const r = get(runId);
      const c = stateAt(r.recs, r.recs.length).confirm;
      if (!c || c.gateId !== gateId || c.answer)
        throw new AppError('E_VALIDATION', 'that request was already answered');
      if (answer === 'deny') {
        push(r, 'confirm.resolved', { answerGate: 'denied' });
        push(r, 'task.finished', { finished: 'cancelled' });
      } else {
        if (answer === 'remember') {
          const t = task(r.run.taskId);
          t.approvals = [
            ...(t.approvals ?? []),
            {
              id: `approval-${seq++}`,
              intent: 'intent.quit_app',
              move: 'shortcut.cmd+q',
              app: 'com.apple.TextEdit',
              items: PREVIEW.length,
              unsaved: 1,
              grantedAt: now(),
            },
          ];
        }
        push(r, 'confirm.resolved', {
          answerGate: answer === 'remember' ? 'remembered' : 'approved',
        });
        push(r, 'confirm.reverified', { reverified: true });
        execute(r);
      }
      sync(r);
      return view(r);
    },
    // ADR 0116 / 0129: the decision model. The mock never goes online: a yes "installs" the pinned model at once
    // (the Rust backend downloads and checks it), and the browser has no Core ML, so the built-in rules still
    // decide, like any build without the runtime.
    'ctrl:model': () => ({ ...model }),
    'ctrl:model-consent': ({ consent }) => {
      model = consent
        ? {
            ...model,
            consented: true,
            state: 'installed',
            bytesOnDisk: model.sizeBytes,
            error: null,
          }
        : {
            ...model,
            consented: false,
            state: 'off',
            error: null,
            bytesOnDisk: 0,
          };
      return { ...model };
    },
    // ADR 0117: put a finished cleanup back. Files that moved since stay; without `rest` that refuses.
    'ctrl:undo': ({ runId, rest }) => {
      const r = get(runId);
      const u = stateAt(r.recs, r.recs.length).undo;
      if (!u)
        throw new AppError('E_VALIDATION', "this run didn't move any files");
      if (u.done)
        throw new AppError('E_VALIDATION', 'this run was already put back');
      const row = (kind: string, extra: Omit<Rec, 'row'> = {}) =>
        r.recs.push({
          row: {
            seq: r.recs.length,
            kind,
            at: now(),
            actionId: 'a0',
            node: null,
          },
          ...extra,
        });
      if (u.cantUndo.length && !rest) row('undo.refused');
      else row('undo.done', { undone: u.ready });
      sync(r);
      return view(r);
    },
    'ctrl:revoke-approval': ({ taskId, approvalId }) => {
      const t = task(taskId);
      if (!t.approvals?.some((a) => a.id === approvalId))
        throw new AppError('E_NOT_FOUND', 'that task or run is gone');
      t.approvals = t.approvals.filter((a) => a.id !== approvalId);
      seq++;
      return t;
    },
  };
}
