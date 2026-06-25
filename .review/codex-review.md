# Codex design review — Dashboard share-link copy

**Date:** 2026-06-25  
**Command:** `codex review --title "Dashboard share-link copy (design review)"`  
**Verdict:** Conditionally ready to implement. No CRITICAL blockers.

## Checklist

| # | Item | Result | Severity |
|---|------|--------|----------|
| 1 | Token correctness: share token only | PASS with changes | IMPORTANT |
| 2 | Scope gate: `AccessScope::Owner` only | PASS | IMPORTANT |
| 3 | XSS safety | PASS with changes | IMPORTANT |
| 4 | URL parity with CLI `share_url()` | PARTIAL | IMPORTANT |
| 5 | Regression: share viewers / vouchers | PASS with changes | IMPORTANT |
| 6 | CSS/mobile UX | PASS | MINOR |
| 7 | Newest-token selection semantics | PASS with changes | IMPORTANT |

## Key findings

1. Do not derive copy URL from `query.get("token")` — can be `OWNER_TOKEN`.
2. Keep `token → plan_id` for auth; add separate `plan_slug → newest token` for copy.
3. With `ORDER BY created_at DESC`, use `entry(...).or_insert(token)` not plain `insert()`.
4. URL: trim `PUBLIC_ORIGIN`, hyphenate slug (`_` → `-`), match CLI shape.
5. Voucher links: keep passing request token to `summary::render()`.

## Recommendation

Ready to implement if dual-map auth is preserved and newest-token selection is fixed.