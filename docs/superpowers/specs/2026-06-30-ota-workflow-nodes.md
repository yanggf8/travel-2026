# Design: OTA processing as composable process-nodes + per-source DB config

**Date:** 2026-06-30 · **Status:** DRAFT — Codex-reviewed (ACCEPT-WITH-CHANGES, folded in below). No
code yet. **Author:** Claude (with Yang).

## Codex review (2026-06-30): ACCEPT-WITH-CHANGES

Codex (gpt-5.5, read-only) reviewed this spec; I independently corroborated every load-bearing
finding against source. Result: the navigation-tableable / extraction-stays-agent **seam is SOUND**
(the repo already operates that way), and the premises check out:

| Claim | Verdict | Evidence |
|-------|---------|----------|
| regex/custom parser deleted in code | CONFIRMED | `ota/{parse,regex_parse,settour_parse}.rs` absent; `ota/mod.rs:20` `"parse"` fail-louds; `repo::parser_rules` gone. **DB caveat:** the `parser_rules` *table* still exists in live Turso (intentional — orphaned, not dropped). |
| `ota_source_coverage` columns | CONFIRMED | source_id/product_type/proven/proven_at/method/search_url/blocked_reason_code (+updated_at) — live `PRAGMA table_info` matches. |
| job lifecycle exists | CONFIRMED | `ota_jobs`/`ota_job_params`/`ota_attempts`; enqueue/claim/write-offers wired; live rows present. |
| no-JSON / child-rows convention | CONFIRMED (not perfectly literal) | CLAUDE.md:67-70; child-row migrations. Caveat: `destinations.primary_airports TEXT -- "JSON array"` holds CSV (`NRT,HND`), packed scalar — doesn't affect this design. |

**The 3 required changes, all applied below:**
1. **`param_map` was undefined** (node-1 config referenced it; no table for it). → **DROPPED** —
   `url_template` `{placeholders}` interpolate by-name from `ota_job_params` (+ a small fixed set like
   `{dest_code}`←`region_code`); add an `ota_source_param_map` child table only if a real source ever
   needs a non-identity rename.
2. **`extraction_hint` could become `parser_rules` by another name.** → renamed
   **`agent_extraction_note`** with a hard "deterministic code must never consume it" rule.
3. **Form-step vocabulary too naive to freeze; fixed `settle_ms` flaky.** → enum expanded
   (`wait_for_text_present/absent`, `select_tab`, `url_contains`, `scroll`); `settle_marker` preferred
   over bare `settle_ms`; the form schema is **NOT FROZEN until validated against a real form
   (besttour)**.

## The idea (Yang's framing)

> *"If we have a good design workflow for [OTA sources], what we do is just combine them with the
> suitable process node — with a program or configuration or lookup table in DB. Then what's left
> would be just DB."*

Today, processing an OTA source still carries scattered per-source **knowledge**: the results-URL
shape, "is it a direct GET or a form-drive," "wait ~25s for the async price SPA," "which tab to
capture," "this source ignores its own GET dates." Some lives in `ota_source_coverage.search_url`,
some in docs/memory, some only in the agent's head while it runs.

**Goal:** make the workflow a fixed sequence of **generic process-nodes**, so a source becomes a
**row of config** that parameterizes those nodes — not code. Adding/processing a source becomes a
data operation. *What's left is just DB* — consistent with the project's no-JSON / facts-in-DB rule.

## The hard-won boundary (what is and isn't tableable)

We just DELETED the per-source extraction config (`parser_rules`) because brittle config encoded
extraction WRONGLY (the regex `parse_settour` divided the un-taxed total by pax, grabbed UI chrome
as the hotel). So the design must respect a sharp seam:

- **Deterministic "get to + capture the results page" → tableable.** URL template, GET-vs-form,
  the search params, settle strategy, which tab. These are mechanical and stable per source.
- **Judgment "read the page text → the right offer fields" → STAYS the agent.** Which number is the
  per-person-with-tax price, what's the real hotel vs. UI chrome, what dates the page actually shows
  (vs. what we asked). This is exactly what the agent is good at and config is bad at.

So: **table the navigation/capture; keep extraction agent-first.** The "just DB" end-state applies
to *getting the capture*, not to *parsing it*. (That's the lesson of the `parser_rules` deletion,
re-applied one layer up.)

## The process-nodes (generic, fixed sequence)

Each node is a thin generic step; gwebcdb already provides the primitives (`bridge/*.py`). A run
threads a source-config row through them:

| # | Node | Generic action | gwebcdb primitive(s) | Config it reads |
|---|------|----------------|----------------------|-----------------|
| 0 | **claim** | enqueue + claim a token-guarded job | `travel ota enqueue`/`claim` | source_id, product_type, search params |
| 1 | **resolve-url** | build the results URL from `url_template` by interpolating `{placeholders}` by-name from the job's `ota_job_params` | (pure) | `url_template` |
| 2 | **navigate** | open the URL (or, for `form` kind, run the search form) | `navigate.py` (+ `form_fill`/`combo_select`/`form_click`) | `nav_kind`, `form_steps` |
| 3 | **settle** | wait for async price/hotel render — prefer the `settle_marker` (capture once the loading placeholder is gone); `settle_ms` is a fallback cap | (poll/sleep) | `settle_marker`, `settle_ms` |
| 4 | **capture** | land UNREDACTED page text → `captures` | `ota_capture.py --source --url-contains` | `capture_url_contains` |
| 5 | **extract** | **AGENT reads `captures.raw_text` → offers TSV** | *(the agent — not a node)* | `agent_extraction_note` (human/agent-read guidance, optional) |
| 6 | **write** | persist TSV → `offers` + provenance + attempt audit | `travel ota write-offers` | (none) |
| 7 | **record** | mark coverage proven / blocked | `travel ota finish`, `set-ota-coverage` | (none) |

Nodes 0–4, 6–7 are **deterministic and config-driven**. Node 5 is the agent seam — the one
irreducibly-judgment step. A node can also **fail to a block**: navigate/settle hitting a captcha or
login wall → record `blocked_reason_code`, escalate to the human (agent-first, escalate-on-block).

## The config table — `ota_source_workflow`

Relates to (does not duplicate) `ota_source_coverage` (which stays the *status/result* matrix:
proven/proven_at/method/blocked_reason_code). The new table holds the *how-to-capture* recipe,
keyed the same `(source_id, product_type)`:

> ⚠️ **Form schema NOT FROZEN — and now UNPROVEN.** **Validation finding (2026-06-30):** all 3 sources
> verified live so far (settour, eztravel, **besttour**) are **direct GET** — none needed form-driving.
> So `nav_kind='form'` + `ota_source_form_step` have **zero proven need**. **Recommendation: build the
> GET-only path first** (`nav_kind` can be a fixed `'get'`, or even omitted) and **DEFER the entire form
> sub-schema** until a real source actually requires a form-drive. The `action` enum below stays a
> sketch, not a build target, until then.

```
ota_source_workflow
  source_id            TEXT NOT NULL
  product_type         TEXT NOT NULL
  nav_kind             TEXT NOT NULL CHECK(nav_kind IN ('get','form'))   -- direct URL vs form-drive
  url_template         TEXT            -- e.g. 'https://fit.settour.com.tw/product/v2?depDate={depart},{return}&...'
  capture_url_contains TEXT            -- substring to pick the right tab for ota_capture
  settle_marker        TEXT            -- PREFERRED: text whose ABSENCE means "still loading" — capture
                                       --   only once it's gone (e.g. settour '正在努力查詢' / eztravel 'TWD --')
  settle_ms            INTEGER DEFAULT 0   -- fallback cap when no marker is available
  agent_extraction_note TEXT           -- human/agent-read guidance for node 5 (NEVER machine-consumed; see below)
  PRIMARY KEY (source_id, product_type)
  -- form_steps for nav_kind='form' go in a child table (ordered), never a JSON blob:
ota_source_form_step
  source_id, product_type, step_order INTEGER,
  action TEXT CHECK(action IN ('fill','combo_select','click','scroll',
                               'select_tab','wait','wait_for_text_present','wait_for_text_absent',
                               'url_contains')),   -- NOT FROZEN; validate vs a real form first
  selector TEXT, value TEXT          -- value may interpolate {depart}/{dest_code}/… from job params;
                                     --   for wait_*/url_contains, `value` is the text/substring asserted
  PRIMARY KEY (source_id, product_type, step_order)
```

**URL params — by-name identity interpolation (no `param_map`).** `url_template` `{placeholders}`
resolve by NAME from the job's `ota_job_params` (e.g. `{depart}`←`depart_date`, `{return}`←
`return_date`, `{pax}`←`pax`), plus a small fixed derived set (`{dest_code}`←`region_code`). There is
no per-source param-mapping table: the mapping is identity by construction. (An `ota_source_param_map`
child table would be added ONLY if a real source ever needs a non-identity rename — not now; an empty
mapping table is config-for-config's-sake.)

No JSON: `form_steps` are **child rows** (`ota_source_form_step`), not packed strings — same
discipline as the rest of the schema.

### `agent_extraction_note` — guidance, NEVER machine-consumed (the careful part)

This is the one column that could re-become `parser_rules` if abused. **Hard rule: deterministic code
MUST NOT consume it** — no branching on it, no templating, no regex, no field-rule extraction. It is
free-text the human/agent READS, nothing more. It encodes a *known quirk*, e.g.:
- settour: `"per-person price = 每人機加酒含稅$NN,NNN; NOT 機加酒未稅總價÷pax. /product/v2 resolves via GET."`
- eztravel: `"SPA ignores the GET checkin/checkout — record the dates the PAGE shows. per-person = 機 + 酒含稅費 TWD n /人. no flight numbers."`

The agent still extracts; the note just saves it from re-discovering the quirk. If a note ever *needs*
to be parsed by code to be useful, that's the smell that we're rebuilding the regex parser — stop.

## "What's left is just DB" — where it holds, where it doesn't

- ✅ **Holds for nodes 0–4, 6–7.** A new GET-resolvable source = INSERT one `ota_source_workflow`
  row (nav_kind=get, url_template, capture_url_contains, settle_ms) + enqueue. No code.
- ✅ **Holds for form-driven sources** *if* their search form fits the `ota_source_form_step` action
  vocabulary (incl. the wait/assert/tab actions) — INSERT the ordered rows. **Validate the vocabulary
  against one real form (besttour) before freezing the schema** — a fixed `settle_ms` alone is flaky;
  prefer `settle_marker`.
- ⚠️ **Does NOT hold for extraction (node 5).** Always the agent. By design — that's the lesson of
  the parser_rules deletion.
- ⚠️ **Does NOT hold for genuinely-hostile pages.** `renderer_wedge` (liontravel/lifetour),
  `cloudflare` (booking), `captcha` (skyscanner) aren't a config problem — they need a human or a
  different backend. Those stay `blocked_reason_code` rows, out of the happy path.

So the honest version of "just DB": **the deterministic capture pipeline becomes config + generic
nodes; extraction stays the agent; hostile sources stay flagged-and-escalated.** The win is real —
adding a well-behaved source drops from "write/learn a procedure" to "INSERT a row" — without
recreating the brittle-parser mistake.

## What this would replace / build

- **`travel ota run --capture-only <source> <product_type> [search params]`** — threads a
  `ota_source_workflow` row through the config-driven front half (nodes 0–4: claim → resolve-url →
  navigate → settle → capture) and stops, printing the `capture_id` + `agent_extraction_note`. The
  agent then reads the capture, emits TSV, and calls the existing `travel ota write-offers` (nodes
  6–7). **No single fully-automatic `travel ota run`** — the agent-in-the-middle (node 5) means it
  can't be one call; a resumable wrapper is a later option, not the first build.
- Node 7 (**record**) marks coverage proven/blocked; note the live `parser_rules` table remains
  orphaned-not-dropped (consistent with the 2026-06-29 spec's design-pivot banner) — `record` never
  touches it.
- Seed `ota_source_workflow` from the current scattered knowledge: settour + eztravel first (already
  live-verified — their recipes are known), then the queued sources as each is verified.
- Migrate `ota_source_coverage.search_url` → `ota_source_workflow.url_template` (coverage keeps
  status only).

## Open questions

1. **Form-step vocabulary — RESOLVED to validate-before-freeze.** The `ota_source_form_step.action`
   enum (incl. wait/assert/tab) is a first cut; prove it against one real nontrivial form (besttour
   group_tour) before freezing the schema. Likely needs more than the original `fill/combo_select/
   click/wait`.
2. **`travel ota run` — RESOLVED to `--capture-only` + `write-offers`.** Agent-in-the-middle (node 5)
   rules out a single fully-automatic command; the config-driven front half + explicit agent middle +
   existing `write-offers` is the first build. A resume-seam wrapper is deferred.
3. **Settle — RESOLVED to prefer `settle_marker`.** Capture only once the loading placeholder is gone;
   `settle_ms` is a fallback cap (a bare fixed sleep is flaky).
4. **Home crate (OPEN):** the nodes are gwebcdb primitives; the config + job lifecycle + write are
   travel-cli. Proposed: `travel ota run --capture-only` lives in travel-cli and shells the gwebcdb
   bridge tools (as today), reading `ota_source_workflow` from Turso. Confirm before building.
