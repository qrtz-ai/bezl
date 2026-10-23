import Foundation

/// A focused field's text as read through accessibility, cleaned of editor artefacts before echo inserts into it.
/// Offsets are UTF-16 (NSString / CFRange units), like AXSelectedTextRange.
public struct FieldText: Equatable, Sendable {
    public var value: String
    public var location: Int
    public var length: Int
    public init(value: String, location: Int, length: Int) {
        self.value = value; self.location = location; self.length = length
    }

    /// Slate marks an empty line with U+FEFF and draws its placeholder as a real span after it, which Safari/Chrome
    /// expose in AXValue (e.g. "\u{FEFF}\nGuide Devin while it works…"). An empty Slate field therefore reads as
    /// U+FEFF + hint with the caret at 1: treat that as empty. U+FEFF is never real text, so strip it everywhere and
    /// shift the caret/selection to match.
    public static func normalize(value: String, location: Int, length: Int) -> FieldText {
        let bom: unichar = 0xFEFF
        let ns = value as NSString
        if ns.length > 0, ns.character(at: 0) == bom, location == 1, length == 0 {
            return FieldText(value: "", location: 0, length: 0)
        }
        var out = [unichar](); out.reserveCapacity(ns.length)
        var loc = location, len = length
        for i in 0..<ns.length {
            let c = ns.character(at: i)
            if c == bom {
                if i < location { loc -= 1 } else if i < location + length { len -= 1 }
                continue
            }
            out.append(c)
        }
        return FieldText(value: String(utf16CodeUnits: out, count: out.count), location: max(0, loc), length: max(0, len))
    }
}
