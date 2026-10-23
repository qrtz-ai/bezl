import Foundation

/// How echo may treat a focused field's AXValue before inserting into it (QRT-356).
///
/// Slate marks an empty line with U+FEFF and draws its placeholder as a real, non-editable span, so browsers expose
/// an empty Devin box as "\u{FEFF}\n<hint>" (AXPlaceholderValue blank) and a doc with an empty first line as
/// "\u{FEFF}\n<real text>". The two can't be told apart from AXValue, so a value containing U+FEFF is never rewritten
/// wholesale: insertion goes through the caret (selected text / paste), which leaves Slate's markers and hint alone.
public struct FieldText: Equatable, Sendable {
    /// True when the whole-value AXValue write is safe (no U+FEFF markers in the read value).
    public var canOverwriteValue: Bool
    /// True only when the field is known to hold no user text (for context, never for writing).
    public var isEmpty: Bool
    public init(canOverwriteValue: Bool, isEmpty: Bool) {
        self.canOverwriteValue = canOverwriteValue; self.isEmpty = isEmpty
    }

    public static func classify(value: String, placeholder: String?) -> FieldText {
        let bom = "\u{FEFF}"
        guard value.contains(bom) else {
            let ph = placeholder ?? ""
            return FieldText(canOverwriteValue: true, isEmpty: value.isEmpty || (!ph.isEmpty && value == ph))
        }
        // Text after the marker, minus the marker's line break: empty, or a non-empty placeholder, means empty.
        var rest = value.replacingOccurrences(of: bom, with: "")
        if rest.hasPrefix("\n") { rest.removeFirst() }
        let ph = placeholder ?? ""
        let empty = rest.isEmpty || (!ph.isEmpty && rest == ph)
        return FieldText(canOverwriteValue: false, isEmpty: empty)
    }
}
