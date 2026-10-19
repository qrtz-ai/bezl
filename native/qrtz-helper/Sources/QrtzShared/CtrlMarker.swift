// The source user data on every event ctrl posts ("qrtzctrl"). One constant for both readers: ctrl's stop watcher
// (QrtzCtrl `CtrlStopWatch`) ignores its own events by it, and echo's hotkey filter (QrtzEcho `HotkeyFilter`, review
// M-E) never counts a ⌃ carrying it toward ctrl+fn.
public enum CtrlEventMarker {
    public static let value: Int64 = 0x7172_747A_6374_726C
}
