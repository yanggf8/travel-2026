---
name: travel-shared
description: Shared references for travel planning skills (IO contracts, canonical offer schema, OTA registry, state/dirty flags, cascade triggers).
---

# Travel Shared

Use this bundle when creating/updating any travel planning skill under `src/skills/`.

## Agent-First Defaults

- Lead with action: run the next step and report results; ask only for preferences that change the outcome.
- Use `StateManager` (status/dirty/event logging) instead of direct JSON writes.
- Keep the schema contract centralized; avoid re-encoding path strings in multiple files.
- Always end with one clear “next action” (command or state transition).

## References (load as needed)

- `references/io-contracts.md` — common input/output envelope and write-back rules
- `references/canonical-offer.md` — canonical offer model for normalized results
- `references/date-filters.md` — date filter patterns (flex, preferred/avoid)
- `references/ota-registry.md` — OTA source registry fields and normalization expectations
- `references/state-manager.md` — status + dirty flag conventions
- `references/cascade-triggers.md` — trigger names, scopes, and populate/reset behavior
