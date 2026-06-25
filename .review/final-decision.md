# Final decision — Dashboard share-link copy

**Date:** 2026-06-25

## Go / no-go

**SHIPPED** 2026-06-25 — commit `747fa40`, deployed to `trip-dashboard-rs.yanggf.workers.dev`.

## Resolved questions

| Question | Decision |
|----------|----------|
| Q1: Who sees copy button? | **Logged-in owner only.** Recipients open share link with no login. |
| Q3: Include `lang` in copied URL? | **No** — match CLI `share_url()` (recipient gets ZH default). |
| Q4: Copy from index? | **No** — single plan page only (MVP). |

## Implementation gate (router)

```rust
// Copy button when owner is logged into dashboard.
if is_owner_session {
    // build owner_plan_chrome(login, share_token, public_origin, lang)
}
```

Recipients open the copied URL with `?token=<share_token>` — no dashboard login.

## Reviewers

- Codex: conditionally ready (`.review/codex-review.md`)
- Claude: GO (`.review/claude-review.md`)