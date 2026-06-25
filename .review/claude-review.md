I now have everything needed. Key facts confirmed:

- `created_at` is `TEXT NOT NULL`, written as `datetime('now')` → `YYYY-MM-DD HH:MM:SS` UTC, which is lexicographically sortable. So `ORDER BY created_at DESC` is a valid newest-first sort **at second granularity**.
- The router's current share-token query has **no `ORDER BY`** (router.rs:103) — the plan's §3 proposes adding `ORDER BY created_at DESC` plus a second map. This is a real change to the existing query.
- `summary::render(plan, lang, token)` confirms the voucher-token path is separate from the copy URL.

Here is my structured review.

---

# Review: Dashboard one-click share-link copy

## 1. Must-fix items

**M1 — `created_at` tie-break is unreliable; "newest-first" can silently pick a stale token.**
`created_at` is `TEXT` written as `datetime('now')` → second-granularity UTC. Two tokens minted in the same second sort by SQLite's secondary/undefined order, not by recency. More importantly, `plan_share_tokens` has **no auto-increment / rowid-ordering guarantee in the query** and the PK is `token` (random hex), so there is *no* deterministic recency signal finer than the second. For a single-owner repo this is low-probability, but the design states "first-write-wins = newest" as if it were guaranteed. Either (a) accept it explicitly with a comment that second-collision is acceptable, or (b) better: the plan should note that if a plan has multiple tokens, the copy button is picking *a* valid token, not provably *the* newest. **Recommend:** the plan's "newest" framing is over-claimed. Downgrade the language to "a current valid token (newest by created_at, second granularity)" and don't build correctness on strict recency.

**M2 — Adding `ORDER BY created_at DESC` to the router query changes the auth-map build, which must stay order-independent.** The auth map (`token → plan_id`) is built by `shares.insert(t, p)` and is order-insensitive (every token is a distinct key). That's fine. But the plan says "two maps from one query" and reuses the *same* loop. The must-fix is: the implementation must **not** switch the auth map to `or_insert` or otherwise let ordering affect it — only the new `plan_slug → token` map uses `or_insert`. The plan's §3 wording is correct, but it's the single highest-risk spot for a regression (mixing the two maps' insert semantics). Flag it as a guarded change with a test that the auth map still resolves every token regardless of order.

There are **no other must-fix blockers.** The security model is sound (see §6).

## 2. Should-fix items

**S1 — The `render_plan` signature change ripples to all callers/tests, and the plan understates it.** §"Files to change" says `render_plan(..., owner_chrome: &str)`. Today there is exactly one caller (router.rs:207) plus the test at render/mod.rs:137. Passing a pre-rendered `&str` couples the router to render internals. **Cleaner:** pass `scope: &AccessScope` (or a small `OwnerChrome` option) and let `render_plan` call `share::owner_plan_chrome(...)` internally — keeps URL-building and gating in the render layer where `esc`/`esc_url_attr` live. Either works; the plan should pick one and note the test signature update (it already does, briefly).

**S2 — URL building should reuse `esc_url_attr`, not a new escaper.** The plan says `data-copy-url` is escaped with `esc()`. For a URL in a double-quoted attribute, the repo's convention is `esc_url_attr()` (preserves `&` in the query string, neutralizes `"`/`<`/`>`/space). Using `esc()` would turn `&token=` into `&amp;token=` inside the attribute — which is *correct* for HTML-attribute encoding and the browser will decode it back to `&` when JS reads `dataset.copyUrl`. So `esc()` actually works here. **But** the plan should state explicitly which one and why, because mixing the two conventions is exactly the kind of thing that produced the old `&amp;amp;` double-escape bug noted in `esc()`'s own doc comment. Recommend `esc()` on the full URL value for the attribute, and a unit test asserting the copied string round-trips to a single `&`.

**S3 — `PUBLIC_ORIGIN` is read via `env.secret(...)` and the plan assumes it's always present.** router.rs:58 already reads it (`?` propagates). If `PUBLIC_ORIGIN` is unset the whole request 500s *before* reaching the plan view, so the copy feature can't make it worse — but the new code in `share.rs` should take the already-resolved `public_origin: &str` from the router rather than re-reading the secret, to avoid a second fallible read and keep `share.rs` pure/testable. The plan's file table implies router passes it down; make that explicit.

**S4 — Missing-token UX placement.** §5 says show a muted hint when no share token exists. Good. But the owner may legitimately have *zero* tokens for the currently-viewed plan while having tokens for others (the `plan_slug → token` map is keyed per plan). Confirm the hint is keyed on "no token **for this plan**," not "no tokens at all." The plan's per-plan map handles this correctly; just call it out in the test matrix.

**S5 — First inline `<script>` in read mode sets a precedent.** §6 is careful (dataset, no interpolation, `textContent` flash). One addition: the page shell (`render::page`) has no CSP header today. Adding inline JS without a CSP is consistent with current posture (no CSP exists), but worth a one-line note that this is the first script and a future CSP would need a hash/nonce. Not a blocker.

## 3. Questions to clarify

1. **Q1 — Transitional `OWNER_TOKEN` path: does the owner loading via `?token=<OWNER_TOKEN>` actually need the copy button?** §7 says "OWNER_TOKEN fallback: copy button only (no logout)." But the `OWNER_TOKEN` branch is being removed in the 2nd OAuth-cutover deploy (per the 06-23 plan, step 6). Building/maintaining a distinct chrome variant for a path that's about to be deleted may be wasted effort. Is the `OWNER_TOKEN` variant worth it, or should the copy button render **only** for OAuth sessions (`session_login.is_some()`)? Note the design gates on `AccessScope::Owner` (matches both), so this is a deliberate choice — confirm it's intended given the imminent removal.

2. **Q2 — What does the button copy if the owner is viewing a plan that has *no* share token?** The plan says "show a muted hint." Confirmed there's no button at all in that case (vs. a disabled button)? A disabled button that says "mint one with CLI" might be clearer than a hint that's easy to miss.

3. **Q3 — Does the copied URL need to preserve `lang`?** The owner may be viewing `?lang=en`. The CLI `share_url()` never adds `lang` (defaults to ZH for the recipient). Should the copy match CLI exactly (no lang, ZH default) — which the plan's "URL parity with CLI" implies — or honor the owner's current lang for the recipient? Parity with CLI says drop it; confirm.

4. **Q4 — Index page is explicitly out of scope.** Owner sees the plans index at `/`. No copy affordance there is fine for MVP, but confirm the owner is expected to drill into each plan to copy — not copy from the index list. (Agree this is correct MVP scoping; just confirming.)

## 4. Agreement / disagreement with Codex (point by point)

| Codex finding | My verdict |
|---|---|
| **#1 Don't derive copy URL from `query.get("token")` — can be `OWNER_TOKEN`** | **Strongly agree.** This is the core security correctness point. Confirmed: `auth::resolve` maps `?token=` to `Owner` when it equals `OWNER_TOKEN` (auth.rs:17), so `query.get("token")` can absolutely be the owner secret. Must build from the share-token map. |
| **#2 Keep `token → plan_id` for auth; add separate `plan_slug → newest token`** | **Agree.** Confirmed router.rs:106–116 builds the auth map and it must stay intact. See my **M2** — the risk is in *how* the two maps share the loop, not in the idea. |
| **#3 With `ORDER BY created_at DESC`, use `entry().or_insert()` not `insert()`** | **Agree with a caveat (M1).** The logic is right *given* DESC ordering. But the current router query has **no ORDER BY** at all (router.rs:103) — Codex's note implicitly requires adding it. And `created_at` is second-granularity TEXT, so "newest" isn't provably exact. Codex over-trusts the recency guarantee. Correct mechanism, slightly over-stated precision. |
| **#4 URL: trim `PUBLIC_ORIGIN`, hyphenate slug, match CLI shape** | **Agree.** Confirmed CLI `share_url()` (share_token.rs:34–37) does `plan_id.replace('_', "-")` and `https://{host}/?plan={slug}&token={token}`. CLI does **not** currently trim a trailing slash from its host (it uses a bare const host) — so "trim trailing slash" is a *new* robustness step for the Worker's `PUBLIC_ORIGIN`, not literal parity. Good addition; just note it's Worker-only hardening, not mirroring existing CLI code. |
| **#5 Voucher links: keep passing request token to `summary::render()`** | **Agree, confirmed.** `summary::render(plan, lang, token)` (summary.rs:96) takes the request token for voucher PDF links, which are auth-gated at `/voucher/*` (router.rs:143–147). The copy URL is entirely separate. No regression as long as router.rs:207's `token` arg to `render_plan`/`summary::render` is untouched. |
| **Verdict: conditionally ready, no CRITICAL blockers** | **Agree.** No critical blockers. My M1/M2 are "must-fix in the plan's framing/implementation guidance," not architectural blockers. |

**Where I go beyond Codex:** Codex did not flag (a) the second-granularity `created_at` precision issue (M1), (b) the `render_plan` signature/coupling choice (S1), (c) the `esc()` vs `esc_url_attr()` convention ambiguity (S2), (d) the `OWNER_TOKEN`-soon-to-be-removed question (Q1), or (e) the missing CSP note for the first inline script (S5). Codex's review is accurate but narrow — it validated the security/URL points and missed the maintainability/edge questions.

## 5. Go / No-Go

**GO — implement, with the plan amended on two points first.**

The design is architecturally sound, fits the existing three-tier auth model exactly, introduces no new write endpoints, keeps the share path logged-out-friendly, and correctly isolates the copy URL from owner credentials. It's appropriately MVP (single plan page, CLI-only minting, no over-engineering).

Before coding, amend the plan to:
1. **Soften the "newest token" guarantee** (M1) — `created_at` is second-granularity; the button picks a valid current token, exact-newest isn't guaranteed.
2. **Call out the dual-map insert-semantics split explicitly** (M2) and add a regression test that the auth map still resolves every token regardless of `ORDER BY`.

And resolve **Q1** (is the `OWNER_TOKEN` chrome variant worth building given its imminent removal?) before writing the two-variant chrome — that decision changes how much code §7 actually needs.

No files were modified.
