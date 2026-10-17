//! JavaScript string semantics the TS reference relies on: UTF-16 lengths and cuts, `\s`, `trim()`.

/// `s.length` in JS (UTF-16 code units).
pub fn u16len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `s.slice(0, n)` that never leaves half a surrogate pair (TS `cutSafe`, N21): a character that would straddle
/// unit `n` is left out whole.
pub fn cut_utf16(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}

/// JS `\s` (and what `String.prototype.trim` strips): WhiteSpace + LineTerminator.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// The JS `\s` class body for regexes.
pub const JS_SPACE_CLASS: &str =
    r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// `String.prototype.trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16() {
        assert_eq!(u16len("a😀"), 3);
        assert_eq!(cut_utf16("a😀b", 2), "a");
        assert_eq!(cut_utf16("a😀b", 3), "a😀");
        assert_eq!(cut_utf16("ab", 5), "ab");
        assert_eq!(js_trim("\u{FEFF} x\u{3000}"), "x");
    }
}
