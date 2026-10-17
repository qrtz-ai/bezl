//! Tiny JS-semantics helpers so ports stay byte-for-byte with the TS (`String.prototype.trim`, `\s`,
//! `Math.round`).

/// JS `\s` / `trim()` whitespace (WhiteSpace + LineTerminator). Differs from Unicode `White_Space`
/// (JS includes U+FEFF, excludes U+0085).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// `Math.round` (ties toward +∞).
pub fn js_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_js() {
        assert_eq!(js_trim("\u{feff} a b\u{3000}"), "a b");
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
        assert_eq!((js_round(2.5), js_round(-2.5), js_round(0.49)), (3, -2, 0));
    }
}
