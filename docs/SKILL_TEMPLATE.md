# Skill Template

> Template for creating consistent skill documentation

## Overview

Brief description of what this skill does and when to use it.

## Input Schema

```typescript
interface SkillInput {
  // Define expected inputs
}
```

## Output Schema

```typescript
interface SkillOutput {
  // Define outputs/side effects
}
```

## CLI Commands

```bash
# Primary command
npm run skill -- <command> [options]

# Examples
npm run skill -- example-command --param value
```

### Command Reference

| Command | Description | Required Args | Optional Args |
|---------|-------------|---------------|---------------|
| `command-name` | What it does | `--arg` | `--opt` |

## Workflow Examples

### Example 1: Common Use Case

```bash
# Step 1: Do something
npm run skill -- step1

# Step 2: Do something else
npm run skill -- step2
```

### Example 2: Alternative Flow

```bash
# Alternative approach
npm run skill -- alternative
```

## Error Handling

| Error | Cause | Solution |
|-------|-------|----------|
| `Error message` | Why it happens | How to fix |

## State Changes

Documents what state/data this skill modifies:

- **travel-plan.json**: Which processes/fields
- **state.json**: Which events emitted
- **Cascade triggers**: Which processes marked dirty

## Dependencies

- Required files: `data/destinations.json`
- Required processes: P1 (dates) must be confirmed
- External tools: None

## Notes

Additional context, gotchas, or tips for using this skill.
