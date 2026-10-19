# ctrl changelog

## 0.2.2 (2026-10-19)

- helper: fix QRTZ_MODELS_DIR optional unwrap in Models.swift (ADR 0212) (8a84a06)
- modules: every unit declares its model reads and grants derive from them (ADR 0212, supersedes ADR 0157 §9) (2313447)
- ctrl run 55: "turn on dock magnification" / "turn off magnification on the dock" / "enable dock magnification" sets the Dock's magnification through System E… (ea16cf0)
- ctrl run 54: "make the dock smaller" / "make the dock icons bigger" / "shrink the dock" moves the Dock's size one fixed step through System Events (ADR 0200)… (fc07f63)
- ctrl run 53: "switch the sound to my airpods" / "play sound through the macbook speakers" makes the one connected output device of that kind the sound output… (0c879a3)
- ctrl run 52: "hide the menu bar" / "always show the menu bar" / "stop hiding the menu bar" sets the menu bar's auto-hide through System Events (ADR 0197). Ne… (5dc1f07)
- ctrl run 51: "move the dock to the left" / "put the dock on the right" / "dock on the bottom" sets the Dock's screen edge through System Events (ADR 0195). N… (9f66427)
- ctrl run 50: "keep my mac awake for 2 hours" / "let my mac sleep again" hold and release one timed IOKit power assertion in the helper (ADR 0194). New intent… (f00239b)
- ctrl run 49: "turn off my wifi" / "turn wi-fi back on" sets the Wi-Fi radio through CoreWLAN in the helper (ADR 0191). New wifi_power intent (wifi/wi-fi/wi f… (5671bb3)
- ctrl run 48: minimize / un-minimize are recognised and held until a Mac probe shows an AXMinimized write raises nothing (ADR 0187). Before: "unminimize slack… (fbf0eed)
- ctrl run 47: "stop the screensaver" (ADR 0178 amendment 1: System Events stop current screen saver, one move, no gate, verify ScreenSaverEngine gone, a passw… (5f33378)
- ctrl: run 46, "put my mac to sleep" asks the power manager for a system sleep in the helper (ADR 0183), and Laya's Core ML interface is checked against the p… (8543644)
- ctrl: run 45, "turn off my display" puts the displays to sleep in the helper (ADR 0180). New display_sleep intent (display/screen/monitor + sleep/asleep/off,… (942bf30)

## 0.2.1 (2026-10-19)

- ctrl: run 43, "start the screensaver" starts the current screen saver through System Events (ADR 0178). New screensaver intent (screensaver / screen+saver +… (02c8ab6)
