# Rust CLI Migration Plan

**Status:** ✅ DONE / superseded — the npm→Rust cutover is complete. Root `package.json` is
retired; the Rust CLI (`./bin/travel`) is the sole write path; the TS CLI is read-only under
`archive/ts-cli-retired/`. (This early plan assumed an npm-wrapper-with-Rust-first interim; the
project went all the way to npm-free — see `2026-06-10-roadmap-v2-rust.md` + CLAUDE.md "CLI Execution".)
**Owner:** yanggf

## Goal
Migrate TypeScript CLI to Rust binaries. Preserve npm script interface; Rust runs first when present.

## Execution Priority (Final State)
```
npm script (entrypoint)
  ├── Rust binary first  → ./bin/<tool>          (if -x exists)
  └── TypeScript fallback → ts-node src/...      (always present)
Python/other → explicit `scraper:*` namespace only (forced, never default)
```

---

## 1. Full Command Inventory (travel binary)

The `travel` binary dispatches **55 subcommands** across **33 modules** imported by `src/cli/travel-update.ts`:

| Category | Example Commands | Count (approx) |
|----------|------------------|----------------|
| Views | `status`, `itinerary`, `transport`, `bookings`, `plans` | ~8 |
| Mutations | `set-dates`, `set-flight`, `set-hotel`, `set-airport-transfer` | ~10 |
| Activities | `activity`, `session`, `route`, `day`, `mark-booked` | ~8 |
| Offers | `offers`, `add-offer`, `add-besttour-offer`, `add-lifetour-offer`, `search-compare` | ~6 |
| Shaping | `shaping-*` (init/compare/adopt/baseline) | ~6 |
| Tour-group | `tour-group-*` | ~4 |
| Scraping | `scrape-package`, `scaffold-itinerary`, `populate-itinerary` | ~4 |
| Ops | `weather`, `ops`, `validate`, `chat-format`, `turso`, `query-destination-ref` | ~9 |

**Exact list:** run `./bin/travel --help` after registry loads all 33 modules. Each `CommandHandler` registers 1+ names via `registerCommand()`.

**Note:** `cascade.ts` and `calculate-leave.ts` in `src/cli/` are **not subcommands** — they are internal modules (`StateManager` cascade logic, leave calculator). `calculate-leave.ts` is exposed via the standalone `leave-calc` npm script (see §2).

---

## 2. Migration Scope: Migrate vs. Stay TS Forever

### Migrate to Rust
| Binary | TS Sources | Rationale |
|--------|------------|-----------|
| `travel` | `src/cli/travel-update.ts` + 33 command modules | Core daily driver, 55 subcommands |
| `travel-validate` | `scripts/validate-data.ts` | Data integrity checks run often |
| `travel-compare` | `src/cli/compare-*.ts` | User-facing comparison workflows |
| `travel-utils` | `src/utils/{flight-normalizer,leave-calculator}.ts` | Small, pure functions, easy win |
| `travel-db` | `scripts/turso-{status,query,exec,sync-*}.ts` (non-migration) | DB ops used in dev/CI |

### Stay TypeScript Forever (One-Shot / Infrequent)
| Script | Reason |
|--------|--------|
| `scripts/turso-migrate.ts` (64KB) | Schema migration, run once per env |
| `scripts/seed-plans-current.ts` | One-time plan seeding |
| `scripts/fetch-taiwan-holidays.ts` | Annual holiday fetch |
| `scripts/turso-sync-*.ts` (migration/sync only) | Bootstrap scripts |
| `scripts/import-offers-to-turso.ts` | One-off import tooling |
| `scripts/check_playwright.py` | External dependency check (Python) |

**Rule:** Any script whose primary purpose is "run once to bootstrap/migrate/seed" stays TS. Interactive or hot-path commands migrate.

---

## 3. Crate Layout (Recommended)

Single workspace crate with multiple `[[bin]]` targets:

```
rust/
├── Cargo.toml
├── crates/
│   └── travel-cli/
│       ├── Cargo.toml
│       ├── src/
│       │   ├── main.rs          # dispatches to subcommands
│       │   ├── commands/        # 33+ modules mirroring TS commands/
│       │   ├── shared/          # args, plan-resolver, output
│       │   └── state/           # Turso client wrapper, types
│       └── tests/
│           └── parity/          # insta snapshots vs TS output
└── Cargo.lock
```

**Alternative (multi-crate):** `travel/`, `travel-validate/`, `travel-compare/`, `travel-utils/`, `travel-db/` as sibling crates if compile time or dependency isolation matters. Single-crate `[[bin]]` is simpler for shared `libsql` + `clap` usage.

---

## 4. Shared Library Scope

Rust crate exposes:
- **Turso client** — `libsql` async connection, prepared statements, pipeline batching (match TS `TursoRepository`)
- **Plan resolver** — replicate `src/cli/shared/plan-resolver.ts` logic (env var, `--plan-id`, date anchor fallback)
- **Types** — canonical offer, day, activity, booking, status enums (Zod schemas → Rust structs + serde)
- **Output formatting** — JSON + ASCII table renderers (match TS `output/` helpers)

**Do not duplicate** domain logic that already lives in Turso (date anchors, cascade, process status). Rust CLI is a thin command layer over DB.

---

## 5. Incremental Rollout Order

| Phase | Binary | Rationale | Risk |
|-------|--------|-----------|------|
| 1 | `travel-utils` | Pure functions, no DB, easy parity | Low |
| 2 | `travel-validate` | Read-only, deterministic output | Low |
| 3 | `travel-compare` | Comparison logic, snapshot testable | Medium |
| 4 | `travel-db` | DB ops, but non-critical path | Medium |
| 5 | `travel` | 55 subcommands, highest surface area | High |

**Gate:** each phase must pass parity tests before next phase starts. `travel` is deliberately last.

---

## 6. Parity Test Strategy

Use `insta` for snapshot testing:

```rust
#[test]
fn test_status_output() {
    let output = run_cli(&["status", "--plan-id", "tokyo-2026"]);
    assert_snapshot!(output.stdout);
}
```

**Baseline generation:**
1. Run TS command, capture stdout/stderr to `tests/fixtures/`
2. Implement Rust equivalent
3. `cargo insta test` — first run records snapshot, subsequent runs diff
4. Accept only when human confirms parity

**Edge cases to cover:** `--dry-run`, `--verbose`, missing plan, invalid dates, JSON vs ASCII modes.

---

## 7. Rust Dependency Choices

| Crate | Purpose | Notes |
|-------|---------|-------|
| `clap` (v4, derive) | CLI parsing, subcommands, help | Match TS `args.ts` flag semantics |
| `libsql` | Turso HTTP client | Async, matches TS pipeline batching |
| `serde` + `serde_json` | Struct ↔ JSON | Canonical offer, day cards |
| `tokio` | Async runtime | Required by `libsql` |
| `anyhow` / `thiserror` | Error handling | Pragmatic error surface |
| `insta` | Snapshot tests | Parity vs TS baseline |
| `colored` / `termcolor` | Colored terminal output | Optional polish |
| `chrono` | Date handling | Match TS `date-utils.ts` |

**Avoid:** heavy web frameworks, unnecessary proc macros. Keep compile times reasonable for solo dev.

---

## Implementation Steps (Rust Side)

1. Initialize `rust/` crate with layout from §3.
2. Implement `travel-utils` first (phase 1); add `insta` tests.
3. Progress through phases 2–5; each binary must pass parity gate.
4. Add `bin/` to `.gitignore`.
5. Only after all 5 binaries pass: edit `package.json` (see below).

## package.json Edit (Only After All Binaries Pass)

Replace every `ts-node ...` entry with:
```json
"[ -x ./bin/<binary> ] && ./bin/<binary> <args> || ts-node <original>"
```

## Constraints

- **Never edit `package.json`** until Rust migration is complete and all tests pass.
- Python scripts stay in `scraper:*` namespace.
- `./bin/` is gitignored.
- Fallback to TypeScript must remain functional at all times.

## References

- Current `package.json` scripts (pure ts-node)
- `CLAUDE.md` → "CLI Execution Priority (Future Design)"
- `docs/reference/CLI.md` → plan note at top
- `src/cli/travel-update.ts` (33 command imports, 55 subcommands)
- `src/cli/commands/registry.ts`

## Rollback

Delete binaries from `./bin/` — npm scripts automatically fall back to TypeScript. No code change required.