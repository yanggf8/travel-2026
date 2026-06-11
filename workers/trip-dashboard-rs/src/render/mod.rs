pub mod session;
pub mod day;
pub mod map;

/// Escape text for HTML TEXT content and DOUBLE-QUOTED attribute values only.
/// (Escapes & < > ". Not safe for single-quoted attrs, unquoted attrs, URLs, or
/// JS/CSS contexts — build those from trusted components instead.)
/// Escape ONCE — never double-escape (the old TS bug rendered `&amp;amp;`).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a URL for a double-quoted HTML attribute (href/src).
/// Neutralizes attribute-breaking chars (" < > space) via percent-encoding but
/// does NOT touch `&`, so query strings (`?q=a&z=15`) survive intact.
/// Use this for URLs; use esc() for text/non-URL attribute values.
pub fn esc_url_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            ' ' => out.push_str("%20"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_ampersand_once() {
        assert_eq!(esc("Museum & Art"), "Museum &amp; Art");
        assert!(!esc("Museum & Art").contains("amp;amp;"));
    }
    #[test]
    fn esc_url_attr_preserves_ampersand_neutralizes_quotes() {
        assert_eq!(esc_url_attr("https://x/?q=a&z=15"), "https://x/?q=a&z=15"); // & preserved
        assert_eq!(esc_url_attr("https://x/?q=\"a\""), "https://x/?q=%22a%22"); // quote neutralized
        assert!(!esc_url_attr("a b").contains(' ')); // space encoded
    }
}
