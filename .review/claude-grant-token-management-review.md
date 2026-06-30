I note `esc()` does not escape single quotes (`'`). This matters for HTML attribute values that use single-quote delimiters, but all the grant HTML uses double-quote delimiters (`value="..."`), so `esc()` (which escapes `"`) is correct for those. The `created_at`/`deactivated_at` come from `datetime('now')` so they're safe regardless.

I now have everything I need. Let me confirm the one migration-compatibility nuance: SQLite's behavior of `ALTER TABLE ... ADD COLUMN ... NOT NULL DEFAULT 'active'` on a table with existing rows.

That's well-defined: SQLite allows `ADD COLUMN NOT NULL` **only if a DEFAULT is provided** (which it is — `'active'`), and backfills existing rows with the default. So existing rows become `status='active'`. Confirmed correct.

Let me write up the findings.

---

# Grant-Token Management Review — Findings

Reviewed against the live codebase (`auth.rs`, `session.rs` in `worker-github-oauth`, `turso.rs`, `render/mod.rs`, migration helpers). Ordered by severity.

## Findings

### 1. MEDIUM — `created_by` / `deactivated_by` are SQL-escaped but never length-bounded or validated; trusts the OAuth login is safe — confirmed safe, but fragile by construction
`router.rs:646` (`create_grant`) and `router.rs:665` (`deactivate_grant`) interpolate `owner_login` into raw SQL via `sql_quote()` (`router.rs:~700`, `s.replace('\'', "''")`). Since `turso::pipeline` sends **raw SQL with no parameter binding** (`turso.rs:48`, `"stmt": { "sql": sql }`), `sql_quote` is the *only* defense.

This is **currently safe**: `owner_login` is the return value of `gho::verify_session`, which returns the login *only after* `ct_eq(login, allowed_login)` passes (`session.rs:93`) — so it can only ever equal the configured `ALLOWED_LOGIN`. `sql_quote` correctly doubles single quotes, and SQLite doesn't honor backslash escapes, so `''`-doubling is complete.

Residual risk, not a blocker: the safety depends on an invariant two crates away (verify_session pinning login to allow-list). If anyone ever relaxes the OAuth crate to accept org members / multiple logins, this becomes an injection sink with no second line of defense. The code already has `is_safe_slug` and `is_grant_token` validators for the other interpolated values — `created_by`/`deactivated_by` are the only un-allowlisted interpolations. Worth a comment documenting the invariant.

### 2. MEDIUM — No tests cover the actual mutation SQL builders, CSRF gate wiring, or owner-gate on POST routes
`router.rs` added tests only for `is_grant_token` (`router.rs:592`) and `GrantCsrf` round-trip (`router.rs:602`). There are **no tests** for:
- `create_grant` / `deactivate_grant` SQL string shape (the `WHERE EXISTS` guard, the `status='active'` filter on deactivate).
- `sql_quote` (the sole injection defense) — e.g. that `o'brien` → `o''brien`.
- The 403-on-non-owner / 405-on-non-POST gate in `handle` (`router.rs:117–119`).
- `handle_grant_post` dispatch (invalid slug → 400, bad CSRF → 403, missing plan → 404).

These are pure-logic paths extractable without the Worker runtime. Given this is auth/CSRF/SQL code, the absence of unit coverage on the builders and the gate is a deployment risk. Recommend at minimum a `sql_quote` test and tests asserting the SQL contains the `status='active'` guard. **This is the most actionable gap before deploy** — the mutation path is the highest-risk new surface and is entirely untested.

### 3. LOW — `is_grant_token` rejects legacy share tokens for deactivation
`is_grant_token` (`router.rs:683`) requires exactly 32 lowercase hex chars. Existing `plan_share_tokens` rows minted by `./bin/travel share-token` (the pre-existing CLI flow) may not match this format. After migration those legacy rows get `status='active'` (correct — they keep working for viewers), and they'll render in history with a copy button. But the **"Make inactive" button will fail with `400 invalid token`** because `is_grant_token` rejects the legacy format.

So an owner cannot deactivate a legacy share token through the UI — only new 32-hex tokens. Whether this is a regression depends on legacy token format; verify what `share-token` mints. If legacy tokens differ, owners lose the ability to revoke them via the dashboard (they'd need raw SQL). Not blocking if all existing tokens already happen to be 32-hex, but should be confirmed against the live `plan_share_tokens` rows.

### 4. LOW — CSRF token tied to full session cookie; rotates silently on re-login (correct, but a UX edge)
`GrantCsrf` salts the HMAC with the entire session cookie value (`router.rs:~728`, `self.session_cookie`). This is a sound double-submit-via-HMAC design and correctly binds CSRF to the session. The session cookie embeds `exp` (`session.rs:84`) and is stable for the cookie's life, so GET-minted tokens verify on POST.

Edge case: if a session is **re-issued** (re-login refreshes the cookie) between page render and form submit, all CSRF tokens on the stale page become invalid → `403 invalid csrf` on submit. The user sees a generic 403 with no recovery hint (the handler returns `Response::error("invalid csrf", 403)`, `router.rs:619/630`). Functionally safe (fails closed), but a confusing dead-end. Consider redirecting back to `/` instead of a bare 403. Not blocking.

Also note: the session cookie is `SameSite=Lax` (`session.rs:182`). Lax already blocks cross-site POSTs, so the HMAC CSRF is defense-in-depth — good, belt-and-suspenders.

### 5. LOW — `redirect_after_grant` reflects unsanitized `plan` into the `Location` query, and `grant=`/`plan=` are not surfaced
`redirect_after_grant` (`router.rs:672`) builds `Location: /?grant={status}&plan={plan}`. `plan` reaches here only after `is_safe_slug` passed (`router.rs:610`), so it's `[a-z0-9_-]` — no header-injection or open-redirect risk. Fine.

Minor: the `?grant=created` / `?grant=inactive` query params added to the redirect target are **never read or rendered** anywhere (the index handler at `router.rs:187` ignores them). So there's no success/confirmation toast — the owner just sees the list re-render. Cosmetic; flagging because the i18n keys `created`/`inactivated` were added (`i18n.rs`) suggesting an intent to show status that isn't wired to the redirect. No functional impact.

### 6. INFO — `getrandom` 0.2 with `js` feature: verify it resolves to web-crypto, not `wasi`
`Cargo.toml` adds `getrandom = { version = "0.2", features = ["js"] }` and `mint_grant_token` (`router.rs:676`) calls `getrandom::getrandom(...).expect("CSPRNG failed")`. The `Cargo.lock` pulls `getrandom 0.2.17` with both `js-sys`/`wasm-bindgen` **and** `wasi` deps. On the `wasm32-unknown-unknown` Workers target, the `js` feature must win so it uses `crypto.getRandomValues`. The existing OAuth crate uses web-crypto directly (`session.rs:102`, `random_state`) rather than `getrandom`, so this is a *new* RNG path for this codebase.

This should work (the `js` feature is the documented Workers approach for getrandom 0.2), but it's untested in this worker and a `panic!` on failure (`.expect`) would 500 the create route. **Verify with a real `worker-build --release` + a live create** before relying on it — a build that targets `wasm32-wasi` or mis-resolves the feature would panic at runtime, not compile-time. This is the runtime risk most worth a smoke test.

### 7. INFO — Migration is correct for both fresh and existing DBs
- Fresh DB: `PHASE1_TABLES` `CREATE TABLE IF NOT EXISTS plan_share_tokens` now includes all five columns (`db_migrate.rs:1718`). ✓
- Existing DB: four `add_column` calls + index (`db_migrate.rs:423–439`). `ADD COLUMN status TEXT NOT NULL DEFAULT 'active'` is legal in SQLite **because** a DEFAULT is supplied — existing rows backfill to `'active'`, so prior share tokens keep authorizing viewers. ✓ `add_column` tolerates "duplicate column" so re-running migrate is idempotent (`db_migrate.rs:50`). ✓
- The `build_grant_maps` status parser treats `"" | "active"` as Active (`share.rs`), so even if some row's `status` is NULL/empty it's read as active — consistent with the migration default. ✓

No migration blocker.

---

## Verdict

**No findings block correctness/security of the auth model** — owner/viewer separation is sound (owner scope only from verified OAuth session; `?token=` only ever yields `AccessScope::Plan`; index is owner-gated; viewer auth accepts only active tokens via `token_to_plan`), CSRF is properly HMAC-bound to action+plan+token+session, and SQL interpolation is escaped/allowlisted on every path.

**The one thing I'd require before deploy is #2** (no tests on the mutation SQL builders / `sql_quote` / owner+CSRF gate) combined with a live smoke test for **#6** (`getrandom` js-feature in the real WASM build) — the create route panics rather than degrades if the RNG path is misconfigured.

**Residual deployment risks to confirm:**
- Run `worker-build --release` + a real `POST /grants/create` once to prove `getrandom` resolves to web-crypto (not a runtime panic). (#6)
- Confirm existing `plan_share_tokens` rows are 32-hex, or accept that legacy tokens can't be deactivated via the UI. (#3)
- Add a `sql_quote` unit test and assert the deactivate SQL keeps its `status='active'` guard. (#2)

I have not rewritten any implementation, per the request.
