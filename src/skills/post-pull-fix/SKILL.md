---
name: post-pull-fix
description: Automated health check and fix workflow after git pull
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /post-pull-fix

## Purpose

Catch common post-pull breakage: missing dependencies, type errors, merge artifacts, broken CLI.

## When to Use

After `git pull`, `git merge`, or switching branches.

## Workflow

### 1. Install / build

```bash
make setup
```

### 2. Type check

```bash
make check
```

### 3. Check for merge artifacts

```bash
grep -rn "<<<<<<< HEAD" src/ data/ --include="*.ts" --include="*.json" || echo "No conflict markers"
```

### 4. Smoke test CLI

```bash
./bin/travel status --full
```

### 5. Scraping environment

The Python scrapers are decommissioned. OTA capture now runs through the chromeport
CDP driver (`rust/crates/chromeport`) attaching to a real Chrome — no Playwright/Python
setup. See `/scrape-ota` for the capture flow.

## Quick Command

```bash
make setup && make check && ./bin/travel status --full && echo "Post-pull checks complete"
```

## Common Issues

| Issue | Cause | Fix |
|-------|-------|-----|
| 17 type errors | API signature changed | Check git diff, update call sites |
| Cannot find module | File moved/renamed | Search for old import paths |
| Duplicate keys in JSON | Merge conflict | Manually resolve, keep most recent |
| CLI command not found | Command renamed | Check `./bin/travel --help` |

## See Also

- `scripts/validate-data.ts` — Full data validation suite
- `make test` — Integration test suite
