//! Shared test helpers.
//!
//! `tests/common/mod.rs` is the Cargo idiom for code shared across integration test
//! binaries WITHOUT it becoming its own test binary. Each test file opts in with
//! `mod common;` and `use common::Guard;`.

/// RAII teardown guard: runs the wrapped closure on `Drop` — i.e. on BOTH normal
/// return AND panic-unwind.
///
/// Integration tests historically called `teardown(...)` as the LAST statement of the
/// test. A panicking assertion (every TDD RED run, and any real regression) unwinds the
/// stack past that trailing call, so teardown never runs and test rows LEAK into the
/// shared Turso DB. Wrapping teardown in a `Guard` closes that hole: the closure fires
/// during unwinding.
///
/// Usage:
/// ```ignore
/// mod common;
/// use common::Guard;
/// // ... build plan/dest ...
/// teardown(&plan, &dest);                 // optional defensive pre-clean
/// let _g = Guard::new({
///     let (plan, dest) = (plan.clone(), dest.clone());
///     move || teardown(&plan, &dest)
/// });
/// // ... seed, run, assert (any panic still tears down) ...
/// ```
pub struct Guard<F: FnMut()>(F);

impl<F: FnMut()> Guard<F> {
    pub fn new(f: F) -> Self {
        Guard(f)
    }
}

impl<F: FnMut()> Drop for Guard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
