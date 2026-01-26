#!/usr/bin/env node
/**
 * Cascade Runner CLI
 *
 * Usage:
 *   npx ts-node src/cli/cascade.ts                    # Dry-run (default)
 *   npx ts-node src/cli/cascade.ts --apply            # Apply changes in-place
 *   npx ts-node src/cli/cascade.ts --apply --output new.json
 *
 * After compilation:
 *   node dist/cli/cascade.js [options]
 */

import { run, RunOptions } from '../cascade/runner';
import { CascadePlan, CascadeResult } from '../cascade/types';

// ============================================================================
// CLI Argument Parsing
// ============================================================================

interface CliArgs {
  input: string;
  output?: string;
  apply: boolean;
  format: 'text' | 'json';
  verbose: boolean;
  help: boolean;
}

function parseArgs(argv: string[]): CliArgs {
  const args: CliArgs = {
    input: 'data/travel-plan.json',
    output: undefined,
    apply: false,
    format: 'text',
    verbose: false,
    help: false,
  };

  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];

    switch (arg) {
      case '--help':
      case '-h':
        args.help = true;
        break;

      case '--apply':
        args.apply = true;
        break;

      case '--input':
      case '-i':
        args.input = argv[++i];
        break;

      case '--output':
      case '-o':
        args.output = argv[++i];
        break;

      case '--format':
      case '-f':
        const format = argv[++i];
        if (format === 'json' || format === 'text') {
          args.format = format;
        }
        break;

      case '--verbose':
      case '-v':
        args.verbose = true;
        break;

      default:
        if (arg.startsWith('-')) {
          console.error(`Unknown option: ${arg}`);
          process.exit(1);
        }
    }
  }

  // --output only meaningful with --apply
  if (args.output && !args.apply) {
    console.error('Warning: --output is ignored without --apply');
    args.output = undefined;
  }

  return args;
}

function printHelp(): void {
  console.log(`
Cascade Runner - Execute cascade rules on travel-plan.json

USAGE:
  npx ts-node src/cli/cascade.ts [OPTIONS]
  node dist/cli/cascade.js [OPTIONS]

OPTIONS:
  -i, --input <path>   Input file (default: data/travel-plan.json)
  -o, --output <path>  Output file (only with --apply, default: same as input)
  --apply              Apply changes (default: dry-run only)
  -f, --format <fmt>   Output format: text (default) or json
  -v, --verbose        Show detailed output
  -h, --help           Show this help

EXAMPLES:
  # Dry-run: show what would change
  npx ts-node src/cli/cascade.ts

  # Dry-run with JSON output
  npx ts-node src/cli/cascade.ts --format json

  # Apply changes in-place
  npx ts-node src/cli/cascade.ts --apply

  # Apply to a new file
  npx ts-node src/cli/cascade.ts --apply --output data/travel-plan.new.json
`);
}

// ============================================================================
// Output Formatting
// ============================================================================

function formatPlanText(plan: CascadePlan, verbose: boolean): string {
  const lines: string[] = [];

  lines.push('='.repeat(60));
  lines.push('CASCADE PLAN');
  lines.push('='.repeat(60));
  lines.push(`Computed at: ${plan.computed_at}`);
  lines.push(`Triggers evaluated: ${plan.triggers_evaluated.length}`);
  lines.push(`Actions: ${plan.actions.length}`);
  lines.push(`Warnings: ${plan.warnings.length}`);
  lines.push('');

  if (plan.actions.length === 0) {
    lines.push('No actions to perform (all clean).');
  } else {
    lines.push('ACTIONS:');
    lines.push('-'.repeat(60));

    for (const action of plan.actions) {
      switch (action.type) {
        case 'reset':
          lines.push(`[RESET] ${action.destination}.${action.process}`);
          if (verbose) {
            lines.push(`        Reason: ${action.reason}`);
            lines.push(`        Triggered by: ${action.triggered_by}`);
          }
          break;

        case 'populate':
          lines.push(`[POPULATE] ${action.destination}`);
          lines.push(`           ${action.source_path} → ${action.target_path}`);
          if (verbose) {
            lines.push(`           Source marker: ${action.set_source}`);
            lines.push(`           Triggered by: ${action.triggered_by}`);
          }
          break;

        case 'dirty_flag':
          if (verbose) {
            lines.push(`[DIRTY_FLAG] ${action.destination}.${action.process} = ${action.dirty}`);
          }
          break;

        case 'global_dirty_flag':
          if (verbose) {
            lines.push(`[GLOBAL_DIRTY_FLAG] ${action.process} = ${action.dirty}`);
          }
          break;
      }
    }
  }

  if (plan.warnings.length > 0) {
    lines.push('');
    lines.push('WARNINGS:');
    lines.push('-'.repeat(60));
    for (const warning of plan.warnings) {
      lines.push(`  ⚠ ${warning}`);
    }
  }

  if (verbose) {
    lines.push('');
    lines.push('TRIGGERS EVALUATED:');
    lines.push('-'.repeat(60));
    for (const trigger of plan.triggers_evaluated) {
      lines.push(`  - ${trigger}`);
    }
  }

  return lines.join('\n');
}

function formatResultText(result: CascadeResult, verbose: boolean): string {
  const lines: string[] = [];

  lines.push(formatPlanText(result.plan, verbose));
  lines.push('');
  lines.push('='.repeat(60));

  if (result.applied) {
    lines.push(`✓ Changes applied to: ${result.output_path}`);
  } else {
    lines.push('(Dry-run mode - no changes applied)');
    lines.push('Use --apply to apply changes.');
  }

  if (result.errors.length > 0) {
    lines.push('');
    lines.push('ERRORS:');
    for (const error of result.errors) {
      lines.push(`  ✗ ${error}`);
    }
  }

  lines.push('='.repeat(60));

  return lines.join('\n');
}

// ============================================================================
// Main
// ============================================================================

function main(): void {
  const args = parseArgs(process.argv);

  if (args.help) {
    printHelp();
    process.exit(0);
  }

  const options: RunOptions = {
    inputPath: args.input,
    outputPath: args.output,
    apply: args.apply,
  };

  const result = run(options);

  if (args.format === 'json') {
    console.log(JSON.stringify(result, null, 2));
  } else {
    console.log(formatResultText(result, args.verbose));
  }

  process.exit(result.success ? 0 : 1);
}

main();
