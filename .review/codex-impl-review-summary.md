   565	}
   566	.copy-share-btn:active {
   567	  opacity: 0.9;
   568	}
   569	.copy-share-btn.copy-share-ok {
   570	  background: var(--green);
   571	}
   572	.copy-share-missing {
   573	  font-size: 12px;
   574	  color: var(--text-dim);
   575	  font-style: italic;
   576	}
   577	.owner-chrome-logout {
   578	  color: var(--accent);
   579	  text-decoration: none;
   580	  margin-left: auto;
   581	}
   582	.owner-chrome-logout:active {
   583	  text-decoration: underline;
   584	}
   585	
   586	/* Print */
   587	@media print {
   588	  body { max-width: none; padding: 0; }
   589	  .day { break-inside: avoid; }
   590	  .daymap, .planmap { break-inside: avoid; }

codex
**Findings**

IMPORTANT: `build_share_maps()` normalizes underscores only for the copy map, not the auth map. In [share.rs](/home/yanggf/b/travel-2026/workers/trip-dashboard-rs/src/render/share.rs:35), `token_to_plan` stores raw `plan_id`, while `plan_slug_to_token` stores `plan_id.replace('_', '-')`. If a row is `("abc", "okinawa_2026")`, the owner copies `/?plan=okinawa-2026&token=abc`, but the no-login viewer resolves to `AccessScope::Plan("okinawa_2026")` and fails `can_view_plan(..., "okinawa-2026")`. Current hyphenated plan IDs are fine, but this is a real mismatch if underscore rows exist.

MINOR: Newest-token choice is only ordered by `created_at DESC` in [router.rs](/home/yanggf/b/travel-2026/workers/trip-dashboard-rs/src/router.rs:104). Since CLI inserts `datetime('now')`, same-second token mints can tie and choose nondeterministically. Any chosen token is still valid, so this is not a flow blocker.

**Checklist**

| Check | Result |
|---|---|
| Owner logged in opens `/?plan=slug` | PASS |
| Copy button shown only for owner session | PASS |
| Copied URL uses `PUBLIC_ORIGIN`, `plan`, and stored share token | PASS |
| Does not copy `OWNER_TOKEN`, session cookie, or current request URL | PASS |
| No-login viewer can open copied link | PASS for hyphenated `plan_id`; FAIL if token row uses underscore `plan_id` |
| Viewer does not see owner chrome/copy/logout | PASS |
| Voucher links keep request token behavior | PASS |
| Missing share token shows owner-only hint | PASS |
| HTML/script escaping avoids obvious XSS/token interpolation issues | PASS |

**Deploy Verdict**

GO if `plan_share_tokens.plan_id` is guaranteed hyphenated like the dashboard `?plan=` slug.

NO-GO if underscore `plan_id` rows are possible or already present; that would break the exact no-login recipient flow. I did not run tests to honor “do not modify files,” since `cargo test` would write build artifacts.
tokens used
65,478
**Findings**

IMPORTANT: `build_share_maps()` normalizes underscores only for the copy map, not the auth map. In [share.rs](/home/yanggf/b/travel-2026/workers/trip-dashboard-rs/src/render/share.rs:35), `token_to_plan` stores raw `plan_id`, while `plan_slug_to_token` stores `plan_id.replace('_', '-')`. If a row is `("abc", "okinawa_2026")`, the owner copies `/?plan=okinawa-2026&token=abc`, but the no-login viewer resolves to `AccessScope::Plan("okinawa_2026")` and fails `can_view_plan(..., "okinawa-2026")`. Current hyphenated plan IDs are fine, but this is a real mismatch if underscore rows exist.

MINOR: Newest-token choice is only ordered by `created_at DESC` in [router.rs](/home/yanggf/b/travel-2026/workers/trip-dashboard-rs/src/router.rs:104). Since CLI inserts `datetime('now')`, same-second token mints can tie and choose nondeterministically. Any chosen token is still valid, so this is not a flow blocker.

**Checklist**

| Check | Result |
|---|---|
| Owner logged in opens `/?plan=slug` | PASS |
| Copy button shown only for owner session | PASS |
| Copied URL uses `PUBLIC_ORIGIN`, `plan`, and stored share token | PASS |
| Does not copy `OWNER_TOKEN`, session cookie, or current request URL | PASS |
| No-login viewer can open copied link | PASS for hyphenated `plan_id`; FAIL if token row uses underscore `plan_id` |
| Viewer does not see owner chrome/copy/logout | PASS |
| Voucher links keep request token behavior | PASS |
| Missing share token shows owner-only hint | PASS |
| HTML/script escaping avoids obvious XSS/token interpolation issues | PASS |

**Deploy Verdict**

GO if `plan_share_tokens.plan_id` is guaranteed hyphenated like the dashboard `?plan=` slug.

NO-GO if underscore `plan_id` rows are possible or already present; that would break the exact no-login recipient flow. I did not run tests to honor “do not modify files,” since `cargo test` would write build artifacts.
