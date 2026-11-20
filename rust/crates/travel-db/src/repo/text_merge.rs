//! Pure text-merge helpers shared by activity notes and route-segment notes
//! (spec 2026-11-20-cli-itinerary-editing-design.md §3.3). No DB access: callers
//! read the current value inside their own transaction, then call these.

/// Append `add` to `existing` without ever overwriting it.
///
/// * `add` is trimmed; empty after trimming → `Err` (nothing to append).
/// * `existing` NULL / blank → `Some(add)`.
/// * Idempotent: if the (trimmed, non-blank) lines of `add` already appear as a
///   CONTIGUOUS block of lines in `existing`, return `Ok(None)` = "No change"
///   (zero writes). Whole-line block matching, not substring, so a short add
///   like "好" is not swallowed by "很好吃".
/// * Otherwise `existing.trim_end() + sep + add`.
///
/// `sep` is `"\n"` for activities and `"；"` for route-segment notes.
pub fn append_note(
    existing: Option<&str>,
    add: &str,
    sep: &str,
) -> Result<Option<String>, String> {
    let add = add.trim();
    if add.is_empty() {
        return Err("note text to append must not be empty".to_string());
    }
    let existing = match existing {
        Some(e) if !e.trim().is_empty() => e,
        _ => return Ok(Some(add.to_string())),
    };
    let lines = |s: &str| -> Vec<String> {
        s.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    };
    let have = lines(existing);
    let want = lines(add);
    if !want.is_empty() && have.windows(want.len()).any(|w| w == want.as_slice()) {
        return Ok(None);
    }
    Ok(Some(format!("{}{}{}", existing.trim_end(), sep, add)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(e: Option<&str>, a: &str, s: &str) -> Result<Option<String>, String> {
        append_note(e, a, s)
    }

    #[test]
    fn empty_add_fails() {
        assert!(ap(Some("x"), "", "\n").is_err());
        assert!(ap(None, "  \n ", "\n").is_err());
    }

    #[test]
    fn null_or_blank_existing_yields_add_trimmed() {
        assert_eq!(ap(None, " 新 ", "\n").unwrap(), Some("新".into()));
        assert_eq!(ap(Some(""), "新", "\n").unwrap(), Some("新".into()));
        assert_eq!(ap(Some("  \n "), "新", "；").unwrap(), Some("新".into()));
    }

    #[test]
    fn appends_with_separator_and_trims_trailing_ws() {
        assert_eq!(ap(Some("舊"), "新", "\n").unwrap(), Some("舊\n新".into()));
        assert_eq!(ap(Some("舊  \n\n"), "新", "\n").unwrap(), Some("舊\n新".into()));
        assert_eq!(ap(Some("舊"), "新", "；").unwrap(), Some("舊；新".into()));
    }

    #[test]
    fn already_present_single_line_is_noop() {
        assert_eq!(ap(Some("舊\n新"), "新", "\n").unwrap(), None);
        assert_eq!(ap(Some("  新  \n舊"), "新", "\n").unwrap(), None);
        assert_eq!(ap(Some("新"), "新", "；").unwrap(), None);
    }

    #[test]
    fn already_present_contiguous_block_is_noop() {
        assert_eq!(ap(Some("a\nb\nc\nd"), "b\nc", "\n").unwrap(), None);
        assert_eq!(ap(Some("a\n b \nc"), "b\r\nc", "\n").unwrap(), None);
    }

    #[test]
    fn non_contiguous_lines_are_appended() {
        assert_eq!(
            ap(Some("a\nb\nc"), "a\nc", "\n").unwrap(),
            Some("a\nb\nc\na\nc".into())
        );
        // same lines, wrong order
        assert_eq!(ap(Some("a\nb"), "b\na", "\n").unwrap(), Some("a\nb\nb\na".into()));
    }

    #[test]
    fn short_text_is_not_substring_false_positive() {
        assert_eq!(ap(Some("很好吃"), "好", "\n").unwrap(), Some("很好吃\n好".into()));
        assert_eq!(ap(Some("abc\ndef"), "ab", "\n").unwrap(), Some("abc\ndef\nab".into()));
    }

    #[test]
    fn idempotent_after_append() {
        let once = ap(Some("舊"), "新", "；").unwrap().unwrap();
        // "；"-joined note is one line: "舊；新" — re-adding "新" is a different line,
        // so it appends again; callers wanting idempotency re-run with the same
        // full add text against the same single-line value.
        assert_eq!(ap(Some(&once), "舊；新", "；").unwrap(), None);
        let nl = ap(Some("舊"), "新", "\n").unwrap().unwrap();
        assert_eq!(ap(Some(&nl), "新", "\n").unwrap(), None);
    }
}
