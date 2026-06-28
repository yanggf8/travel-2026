# RS dashboard worker — completeness / TS-parity audit (2026-06-29)

**Scope:** `workers/trip-dashboard-rs/` (Rust) vs `workers/trip-dashboard/` (legacy TS).
**Method:** parallel feature inventories of both, then source-verified the deltas.
**Verdict:** the RS worker is a **solid, deployed read-only view layer that EXCEEDS the TS worker
on auth/sharing**, but CLAUDE.md's "**at full TS feature parity**" claim is **overstated** — several
TS render features have no RS equivalent (confirmed 0 hits in RS source). None are crashes; they are
missing/dropped features. One (package-offer pricing) is materially visible to viewers.

## RS EXCEEDS TS (added, not in TS)
- **GitHub OAuth owner login** (`/auth/login|callback|logout`, `worker-github-oauth` crate) — TS had none.
- **Grant/share tokens** with lifecycle: `/grants/create` + `/grants/deactivate`, HMAC-SHA256 CSRF,
  `plan_share_tokens` status/created_by/deactivated_*; grant-manager `<details>` UI; Copy-share-link.
- **Keyless R2 maps** (`/map/*`, validated PNG, placeholder) + **gated vouchers** (`/voucher/*`).
- **Private cache headers** for authed pages (`private, no-store`).
- Render fixes over TS: transfer rendering, meal-pin `｜備案` greedy-regex bug, activity-text anchor logic.

## RS MATCHES TS (parity confirmed)
SSR HTML; ZH-default + `?lang=en`; day cards + colored accents; weather strip incl. **feels-like** +
clothing tips + precip%; transit pills w/ Maps links; meal pills; `render_activity_text`
(HTML-escape, `\n`→`<br>`, labeled map links, bare-URL linkify); pending-booking alerts (meal/non-meal,
urgent flag); transit cheat-sheet; flight Google-search links; hotel + airport transfers; anti-translate
meta; plan index; inlined CSS.

## RS GAPS vs TS (source-verified: 0 hits in RS `src/`)
1. **Package / offer pricing block — MISSING (material).** TS booking summary shows the booked
   package (`plan_offers` source_id+product_code, `plan_offer_date_pricing` TWD price/person + for-2
   total, `plan_offer_selection`). RS reads NONE of these tables → a viewer can't see what package was
   booked or its price. *Highest-value gap.*
2. **`day_landmarks` — MISSING.** TS map-embed lists landmarks; RS has no `landmark` reference.
3. **`airport_transfer_candidates` — MISSING.** TS shows alternative transfer options; RS shows only
   the selected transfer.
4. **`hotel_access_lines` — DIFFERENT PATH (likely OK).** TS reads a dedicated table; RS renders the
   hotel `notes` blob into grouped bullets (`render_notes`). Probably adequate, but it's not the same
   source — verify the access directions actually appear for a real plan.
5. **`activity_tags`, `process_statuses`, `bookings_current`, `destination_config` reads — not used**
   by RS plan view (some genuinely unused; `destination_config` currency drives JP-only rows in TS).
6. **JP-only rows (Visit Japan Web / Japan Tourism links)** — TS adds these when currency=JPY; RS
   (no `destination_config` currency read) — verify presence.

## RS INTENTIONALLY DROPPED (by design, per CLAUDE.md — NOT gaps)
- **Edit mode** (`?edit=TOKEN` + `/api/edit` + `ADMIN_TOKEN`): CLAUDE.md L427 explicitly says
  "legacy TS worker edit mode … (not in `-rs`)". All mutations go through `./bin/travel` (Rust CLI).
- **`/api/plan/<id>` JSON, `?nav=` switcher, `/sw.js` service worker, `/favicon.ico`**: not ported.
  `?nav=`, the JSON API, and favicon are minor; the service worker (offline cache) is a real UX drop
  if anyone relied on it.

## Query/table coverage
RS plan load = **15 SELECTs across 13 tables**; TS = **23 statements across 24 tables**. The gap is
the missing offer-pricing/landmark/candidate tables above, plus tables RS folds differently.

## Recommendation (priority order)
1. **Fix CLAUDE.md** — change "at full TS feature parity" to "TS render parity **except** package-offer
   pricing, day_landmarks, transfer candidates; edit-mode intentionally dropped." Accuracy first.
2. **Add the package-offer pricing block to RS** (the one material viewer-visible gap) — read
   `plan_offers`/`plan_offer_date_pricing`/`plan_offer_selection`, render in `summary.rs`.
3. Decide landmarks / transfer-candidates: port or formally declare out-of-scope.
4. Verify hotel-access + JP-only rows render for a real plan (may already be covered).
