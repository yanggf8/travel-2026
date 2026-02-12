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

### 1. Install dependencies

```bash
npm install
```

### 2. Type check

```bash
npm run typecheck
```

### 3. Check for merge artifacts

```bash
grep -rn "<<<<<<< HEAD" src/ data/ --include="*.ts" --include="*.json" || echo "No conflict markers"
```

### 4. Smoke test CLI

```bash
npm run view:status
```

### 5. Check Python environment (if scrapers used)

```bash
python scripts/check_playwright.py --quiet && echo "Playwright ready" || echo "Run: python scripts/check_playwright.py --install"
```

## Quick Command

```bash
npm install && npm run typecheck && npm run view:status && echo "Post-pull checks complete"
```

## Common Issues

| Issue | Cause | Fix |
|-------|-------|-----|
| 17 type errors | API signature changed | Check git diff, update call sites |
| Cannot find module | File moved/renamed | Search for old import paths |
| Duplicate keys in JSON | Merge conflict | Manually resolve, keep most recent |
| CLI command not found | Command renamed | Check `npm run travel -- --help` |

## See Also

- `scripts/validate-data.ts` — Full data validation suite
- `npm test` — Integration test suite
