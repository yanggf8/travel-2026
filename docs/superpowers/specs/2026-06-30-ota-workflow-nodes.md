# Design: OTA processing as composable process-nodes + per-source DB config

**Date:** 2026-06-30 · **Status:** DRAFT — for review (no code yet). **Author:** Claude (with Yang).

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
| 1 | **resolve-url** | build the results URL from a template + the job's params | (pure) | `url_template`, `param_map` |
| 2 | **navigate** | open the URL (or, for `form` kind, run the search form) | `navigate.py` (+ `form_fill`/`combo_select`/`form_click`) | `nav_kind`, `form_steps` |
| 3 | **settle** | wait for async price/hotel render | (poll/sleep) | `settle_ms`, `settle_marker` (text that means "loaded") |
| 4 | **capture** | land UNREDACTED page text → `captures` | `ota_capture.py --source --url-contains` | `capture_url_contains` |
| 5 | **extract** | **AGENT reads `captures.raw_text` → offers TSV** | *(the agent — not a node)* | `extraction_hint` (guidance, optional) |
| 6 | **write** | persist TSV → `offers` + provenance + attempt audit | `travel ota write-offers` | (none) |
| 7 | **record** | mark coverage proven / blocked | `travel ota finish`, `set-ota-coverage` | (none) |

Nodes 0–4, 6–7 are **deterministic and config-driven**. Node 5 is the agent seam — the one
irreducibly-judgment step. A node can also **fail to a block**: navigate/settle hitting a captcha or
login wall → record `blocked_reason_code`, escalate to the human (agent-first, escalate-on-block).

## The config table — `ota_source_workflow`

Relates to (does not duplicate) `ota_source_coverage` (which stays the *status/result* matrix:
proven/proven_at/method/blocked_reason_code). The new table holds the *how-to-capture* recipe,
keyed the same `(source_id, product_type)`:

```
ota_source_workflow
  source_id            TEXT NOT NULL
  product_type         TEXT NOT NULL
  nav_kind             TEXT NOT NULL CHECK(nav_kind IN ('get','form'))   -- direct URL vs form-drive
  url_template         TEXT            -- e.g. 'https://fit.settour.com.tw/product/v2?depDate={depart},{return}&...'
  capture_url_contains TEXT            -- substring to pick the right tab for ota_capture
  settle_ms            INTEGER DEFAULT 0   -- async-render wait before capture
  settle_marker        TEXT            -- OPTIONAL: text whose ABSENCE means "still loading"
                                       --   (e.g. settour '正在努力查詢' / eztravel a 'TWD --' placeholder)
  extraction_hint      TEXT            -- agent-read guidance for node 5 (NOT a parser)
  PRIMARY KEY (source_id, product_type)
  -- form_steps for nav_kind='form' go in a child table (ordered), never a JSON blob:
ota_source_form_step
  source_id, product_type, step_order INTEGER,
  action TEXT CHECK(action IN ('fill','combo_select','click','wait')),
  selector TEXT, value TEXT          -- value may interpolate {depart}/{dest_code}/… from job params
  PRIMARY KEY (source_id, product_type, step_order)
```

No JSON: `param_map`/`form_steps` are **child rows**, not packed strings — same discipline as the
rest of the schema. `{placeholders}` interpolate from the job's `ota_job_params`.

### `extraction_hint` — guidance, not a parser (the careful part)

This is the one column that could re-become `parser_rules` if abused. Rule: it is **free-text
guidance the agent READS**, never a machine-applied rule. It encodes the *known quirk*, e.g.:
- settour: `"per-person price = 每人機加酒含稅$NN,NNN; NOT 機加酒未稅總價÷pax. /product/v2 resolves via GET."`
- eztravel: `"SPA ignores the GET checkin/checkout — record the dates the PAGE shows. per-person = 機 + 酒含稅費 TWD n /人. no flight numbers."`

The agent still extracts; the hint just saves it from re-discovering the quirk. If a "hint" ever
needs to be machine-parsed to be useful, that's the smell that we're rebuilding the regex parser —
don't.

## "What's left is just DB" — where it holds, where it doesn't

- ✅ **Holds for nodes 0–4, 6–7.** A new GET-resolvable source = INSERT one `ota_source_workflow`
  row (nav_kind=get, url_template, capture_url_contains, settle_ms) + enqueue. No code.
- ✅ **Holds for form-driven sources** *if* their search form fits the `fill/combo_select/click/wait`
  step vocabulary — INSERT the ordered `ota_source_form_step` rows. eztravel/besttour likely fit.
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

- A `travel ota run <source> <product_type> [search params]` orchestrator that threads a
  `ota_source_workflow` row through nodes 0–4, then PAUSES at node 5 handing the agent the
  `capture_id` + `extraction_hint`, then (agent supplies TSV) resumes 6–7. (Agent-first: the CLI
  can't do node 5, so `run` is two calls with the agent in the middle — OR `run --capture-only`
  then `write-offers`, which is just today's flow with the config-driven front half.)
- Seed `ota_source_workflow` from the current scattered knowledge: settour + eztravel first (already
  live-verified — their recipes are known), then the queued sources as each is verified.
- Migrate `ota_source_coverage.search_url` → `ota_source_workflow.url_template` (coverage keeps
  status only).

## Open questions

1. **Is the form-step vocabulary (`fill/combo_select/click/wait`) enough** for besttour/eztravel/
   liontravel's real search forms, or do some need a step type we don't have yet? (Verify against one
   real form before committing the schema.)
2. **`travel ota run` orchestrator vs. keep the steps explicit?** An orchestrator is nicer but the
   agent-in-the-middle (node 5) means it can't be one call. Is `--capture-only` + `write-offers`
   (config-driven front half, explicit agent middle) enough, or do we want the full `run` with a
   resume seam?
3. **`settle_marker` polling vs. fixed `settle_ms`?** A marker (capture only once the loading
   placeholder is gone) is more robust than a fixed sleep but needs a per-source "still-loading"
   string. Worth it, or start with `settle_ms` and add the marker later?
4. **Does this belong in travel-cli or gwebcdb?** The nodes are gwebcdb primitives; the config +
   job lifecycle + write are travel-cli. Proposed: `travel ota run` lives in travel-cli and shells
   the gwebcdb bridge tools (as today), reading `ota_source_workflow` from Turso.
