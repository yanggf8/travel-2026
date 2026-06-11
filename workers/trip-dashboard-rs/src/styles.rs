//! Inlined stylesheet for the SSR page shell. The CSS lives in `styles.css`
//! (ported from the TS worker baseline) and is compiled in via include_str!.
pub const CSS: &str = include_str!("styles.css");
