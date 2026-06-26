# Dashboard grant-token management

**Date:** 2026-06-25  
**Worker:** `workers/trip-dashboard-rs/`  
**Status:** Draft design for review  
**Goal:** Logged-in owner can create, copy, inspect, and deactivate per-plan viewer grant tokens from the dashboard. Recipients still open a copied `?plan=<slug>&token=<grant_token>` link without GitHub login.

## Requirement

Owner workflow:

1. Open the dashboard while signed in through GitHub OAuth.
2. For each travel plan, see a share control.
3. If a valid grant token already exists, copy the current available grant link.
4. If no valid grant token exists, the dashboard prompts the owner to create one.
5. The created grant token stays valid until the owner explicitly turns it inactive.
6. The owner can expand/collapse token management with a disclosure triangle to inspect current and historical tokens and manage them.

Recipient workflow:

1. Receive the copied URL over Messenger or another channel.
2. Open the plan without GitHub login.
3. See only that plan, with no owner chrome and no grant-management controls.

## Terms

- **Owner session:** signed `__Host-td_session` GitHub OAuth cookie for `ALLOWED_LOGIN` + `ALLOWED_GITHUB_ID`.
- **Grant token:** opaque bearer token stored in Turso and scoped to exactly one plan.
- **Active token:** valid viewer token. It grants read access to its plan.
- **Inactive token:** historical/revoked token. It remains in history but no longer grants access.
- **Current token:** newest active token for a plan. The default copy button uses this token.

The database table can stay named `plan_share_tokens` for compatibility, but the UI should call these **grant tokens**.

## Current State

`plan_share_tokens` currently has:

```sql
CREATE TABLE IF NOT EXISTS plan_share_tokens (
  plan_id TEXT NOT NULL,
  token TEXT NOT NULL PRIMARY KEY,
  created_at TEXT NOT NULL
)
```

Worker behavior:

- `?token=` is viewer-only after the OAuth cutover.
- The worker loads all rows from `plan_share_tokens`.
- The copy button chooses the newest token per plan by `created_at DESC`.
- There is no active/inactive state, no management UI, and no worker write route.

## Proposed Behavior

### Dashboard Index

For each plan card on `/` while owner is signed in:

- Plan title/date remains a normal link to `/?plan=<slug>`.
- Primary action:
  - If at least one active grant token exists: `Copy share link`.
  - If no active grant token exists: `Create grant token`.
- Disclosure section using native HTML:

```html
<details class="grant-manager">
  <summary>Grant tokens</summary>
  ...
</details>
```

Expanded content:

- **Current valid token:** newest active token, shown by fingerprint and created time.
- **History:** all tokens newest-first, grouped visually by active/inactive.
- Active token rows have:
  - Copy link button.
  - Make inactive button.
- Inactive token rows show:
  - Fingerprint.
  - Created time.
  - Inactivated time.
  - No copy button.

The full token should not be displayed as visible text. Active copy buttons necessarily include the full viewer URL in `data-copy-url`.

### Plan Page

The existing owner chrome on `/?plan=<slug>` should use the same current-token selection:

- Active token exists: `Copy share link`.
- No active token exists: show `Create grant token` owner action or a link back to the dashboard grant manager.

Prefer adding create/manage controls to the index first; plan-page management can be a second step if scope needs to stay small.

### Token Lifecycle

Valid states:

- `active`
- `inactive`

Rules:

- Creating a token inserts a new row with `status='active'`.
- Deactivating a token updates only that row to `status='inactive'`.
- Tokens are never deleted through the dashboard.
- Viewer auth accepts only rows where `status='active'`.
- The copy button chooses the newest active row for that plan.

Recommendation: allow multiple active tokens per plan. That avoids unexpectedly breaking older Messenger links when the owner creates a newer token. The UI still has a single **current** token: newest active. The owner can manually deactivate older active tokens from history.

## Schema

Migrate the existing table in place:

```sql
ALTER TABLE plan_share_tokens ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE plan_share_tokens ADD COLUMN created_by TEXT;
ALTER TABLE plan_share_tokens ADD COLUMN deactivated_at TEXT;
ALTER TABLE plan_share_tokens ADD COLUMN deactivated_by TEXT;
```

Optional, later:

```sql
ALTER TABLE plan_share_tokens ADD COLUMN label TEXT;
ALTER TABLE plan_share_tokens ADD COLUMN copied_at TEXT;
```

Indexes:

```sql
CREATE INDEX IF NOT EXISTS idx_plan_share_tokens_plan_status_created
ON plan_share_tokens(plan_id, status, created_at DESC);
```

No partial unique index initially because multiple active tokens are useful for real-world sharing and avoids forced revocation.

## Worker Routes

All management routes require owner OAuth session. Viewer share tokens must never reach these routes.

### `POST /grants/create`

Form fields:

- `plan=<slug>`
- `csrf=<token>`

Behavior:

1. Verify owner session.
2. Verify CSRF.
3. Verify safe plan slug and plan existence.
4. Generate a 128-bit random token in the Worker.
5. Insert active row.
6. Redirect to `/?grant=created&plan=<slug>`.

### `POST /grants/deactivate`

Form fields:

- `plan=<slug>`
- `token=<grant_token>`
- `csrf=<token>`

Behavior:

1. Verify owner session.
2. Verify CSRF.
3. Verify safe plan slug.
4. Update matching row:

```sql
UPDATE plan_share_tokens
SET status='inactive',
    deactivated_at=datetime('now'),
    deactivated_by=<owner_login>
WHERE plan_id=<plan_id>
  AND token=<token>
  AND status='active'
```

5. Redirect to `/?grant=inactive&plan=<slug>`.

### Future Route: `POST /grants/reactivate`

Not recommended for the first pass. Reactivating old bearer links can surprise the owner. Prefer creating a new token.

## Turso Access

Current worker secret `TURSO_TOKEN` is treated as the read token in code comments. Grant management needs writes.

Recommended secrets:

- `TURSO_TOKEN` remains read-only for normal rendering.
- Add `TURSO_WRITE_TOKEN` for owner-only grant mutation routes.

The write token is used only after owner-session + CSRF validation. Viewer requests never use it.

## CSRF

State-changing grant routes must not rely only on cookies.

Recommended stateless CSRF:

- Server reads the `__Host-td_session` cookie value.
- Server renders hidden input:

```text
csrf = HMAC_SHA256(SESSION_SECRET, "grant:<action>:<plan>:<session_cookie>")
```

- POST handler recomputes and constant-time verifies.
- `SameSite=Lax` helps, but CSRF token is the primary defense.

## SQL Safety

Worker currently constructs SQL strings for trusted slugs. Grant routes introduce token and owner/session values.

Implementation rule:

- `plan` must pass existing `is_safe_slug`.
- `token` must be server-generated 32 lowercase hex chars or validated as `[a-f0-9]{32}`.
- `owner_login` comes from verified session and should still be SQL-escaped or parameterized.
- Prefer adding Turso statement-args support to `turso.rs` before accepting any free-text fields such as labels.

For v1, avoid owner-editable labels/notes to keep mutation SQL simple and safe.

## Rendering Model

Replace the current dual map with richer structs:

```rust
struct GrantToken {
    token: String,
    plan_slug: String,
    status: GrantStatus,
    created_at: String,
    created_by: Option<String>,
    deactivated_at: Option<String>,
    deactivated_by: Option<String>,
}

enum GrantStatus {
    Active,
    Inactive,
}

struct GrantMaps {
    token_to_plan: HashMap<String, String>,          // active tokens only, for viewer auth
    plan_to_current: HashMap<String, GrantToken>,    // newest active token
    plan_to_history: HashMap<String, Vec<GrantToken>>
}
```

Query:

```sql
SELECT token, plan_id, status, created_at, created_by, deactivated_at, deactivated_by
FROM plan_share_tokens
ORDER BY created_at DESC, token DESC
```

Compatibility:

- Missing `status` during rollout is treated as `active` only until migration is deployed.
- After migration, rely on `status`.

## UI Copy

English:

- `Copy share link`
- `Create grant token`
- `Grant tokens`
- `Current valid token`
- `History`
- `Make inactive`
- `Inactive`
- `No active grant token`

Traditional Chinese:

- `複製分享連結`
- `建立授權連結`
- `授權連結`
- `目前有效連結`
- `歷史`
- `停用`
- `已停用`
- `目前沒有有效授權連結`

## Security Invariants

- Owner access is OAuth-session-only.
- `?token=` never grants owner scope.
- Grant tokens are opaque, random, and plan-scoped.
- Inactive tokens cannot view plans or vouchers.
- Management controls render only for owner session.
- Management routes require owner session and CSRF.
- No full tokens are displayed as text in history.
- No token deletion; deactivation preserves audit history.
- Write-capable Turso secret is used only for owner mutation routes.

## Rollout Plan

1. Migration:
   - Add `status`, `created_by`, `deactivated_at`, `deactivated_by`.
   - Add index.
   - Existing rows default to active.
2. Read path:
   - Update worker share-token query to load status/history.
   - Viewer auth accepts active tokens only.
   - Copy button chooses newest active token.
3. Owner dashboard UI:
   - Render plan-card copy/create action.
   - Render `<details>` grant manager.
4. Mutation routes:
   - Add `TURSO_WRITE_TOKEN`.
   - Add CSRF helper.
   - Add create/deactivate POST handlers.
5. CLI:
   - Keep `travel share-token` for emergency/manual minting.
   - `--show` prints token fingerprints + active/inactive status by default.
   - `--show-full` prints full bearer URLs when a URL must be re-copied.
   - `share-token deactivate <token>` marks an active token inactive.
6. Deploy:
   - Deploy migration.
   - Put `TURSO_WRITE_TOKEN`.
   - Deploy worker.
   - Verify owner dashboard create/copy/deactivate.

## Test Plan

Unit tests:

- Active token maps to plan.
- Inactive token is denied.
- Newest active token is current.
- Older active token still works until deactivated.
- Inactive token appears in history but not auth map.
- Create form is rendered only for owner.
- Deactivate form is rendered only for active tokens.
- CSRF accepts valid token and rejects missing/altered token.

Live smoke:

- Logged-out `/?plan=<slug>` → 403 with owner login link.
- Logged-out `/?plan=<slug>&token=<active>` → 200.
- Logged-out `/?plan=<slug>&token=<inactive>` → 403.
- Logged-in `/` → copy button if active token exists.
- Logged-in `/` with no active token → create button.
- Create grant → redirected dashboard shows new active token and copy button.
- Deactivate grant → same URL stops working.

## Open Questions

1. Should the dashboard allow multiple active grant tokens per plan, or should creating a new token automatically deactivate the previous active token?
   - Recommendation: allow multiple active tokens; newest active is the current/default copy token.
2. Should token labels be in v1?
   - Recommendation: no. Add later after Turso parameter support or robust SQL escaping exists.
3. Should plan-page owner chrome include full grant management or only copy/create?
   - Recommendation: index gets full management; plan page gets copy/create only.
4. Should recipient usage update `last_used_at`?
   - Recommendation: no for v1. It turns viewer GET into a write path and needs rate limiting.

## Implementation Boundary

This design is not yet implementation. It should be reviewed first, especially around:

- Multiple active tokens vs single active token.
- Worker write token exposure and route guards.
- CSRF implementation details.
- Whether the UI should expose management on `/`, `/?plan=`, or both.
