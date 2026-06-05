# AGENTS.md

This file is intentionally thin. See [CLAUDE.md](./CLAUDE.md) for the full project context — schema, data model, cascade rules, repository architecture, dev commands, skill decision tree, OTA sources, dashboard, and CLI quick reference.

## Codex-specific deltas

- **CLI agent first; plain text only.** Follow the shared `CLAUDE.md` rule: prioritize the native CLI agent workflow, make user-facing command output plain text/table lines, and do not add JSON files, JSON fixtures, or JSON as a scraper/importer pipeline boundary. Store structured data in normalized Turso tables, then render plain-text CLI views from Turso.

If a Codex-specific config, env, or workflow ever diverges from what `CLAUDE.md` documents, capture it here so the two files have a single source of truth plus a small delta — instead of two copies to keep in sync.
