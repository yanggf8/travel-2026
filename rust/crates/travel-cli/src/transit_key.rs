//! Single source of truth for `destination_transit` station-pair keys.
//!
//! `derive-routes` looks up transit metadata by `pair_key`, and `add-transit`
//! WRITES it — both MUST agree on normalization or a written pair is never found.
//! Keep the key math here and have both callers use it (do not duplicate).
//!
//! Key form: `{norm(from)}_to_{norm(to)}` where `norm` lowercases and collapses
//! internal whitespace. The lookup also accepts the pipe form and the reverse
//! direction; the primary WRITE key is the forward `_to_` form.

/// Lowercase + collapse internal whitespace: `"  Shinjuku   Gyoemmae "` → `"shinjuku gyoemmae"`.
pub(crate) fn norm_station(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The canonical WRITE key for a directed pair: `{norm(from)}_to_{norm(to)}`.
pub(crate) fn primary_pair_key(from: &str, to: &str) -> String {
    format!("{}_to_{}", norm_station(from), norm_station(to))
}

/// All pair_key forms `derive-routes` accepts, in its established order:
/// forward `_to_`, forward `|`, reverse `_to_`, reverse `|`. Index 0 is the
/// primary write key, so a value written via `primary_pair_key` always hits.
pub(crate) fn lookup_candidates(from: &str, to: &str) -> [String; 4] {
    let a = norm_station(from);
    let b = norm_station(to);
    [
        format!("{a}_to_{b}"),
        format!("{a}|{b}"),
        format!("{b}_to_{a}"),
        format!("{b}|{a}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_station_lowercases_and_collapses_whitespace() {
        assert_eq!(norm_station("  Shinjuku   Gyoemmae "), "shinjuku gyoemmae");
        assert_eq!(norm_station("TOCHOMAE"), "tochomae");
    }

    #[test]
    fn primary_pair_key_is_forward_to_form() {
        assert_eq!(
            primary_pair_key("  Shinjuku   Gyoemmae ", "TOCHOMAE"),
            "shinjuku gyoemmae_to_tochomae"
        );
    }

    #[test]
    fn candidate_zero_equals_primary_write_key() {
        // The invariant that makes add-transit's written key findable by derive-routes.
        let (from, to) = ("Shinjuku Gyoemmae", "Tochomae");
        assert_eq!(lookup_candidates(from, to)[0], primary_pair_key(from, to));
    }

    #[test]
    fn candidates_preserve_derive_order() {
        let c = lookup_candidates("A", "B");
        assert_eq!(c, ["a_to_b", "a|b", "b_to_a", "b|a"]);
    }
}
