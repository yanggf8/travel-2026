/**
 * Status Check Program for Yokohama Travel Project
 * Evaluates readiness_rules and generates status report
 */

import * as fs from 'fs';
import * as path from 'path';
import { evaluateRuleBlock, EvalResult, RuleBlock } from './rule-evaluator';
import { PATHS } from '../config/constants';

interface ProcessStatus {
  name: string;
  displayName: string;
  status: string;
  readyToProceed: boolean;
  currentMilestone: string | null;
  nextMilestone: string | null;
  milestones: { [key: string]: boolean };
  missingFields: string[];
}

interface StatusReport {
  project: string;
  generatedAt: string;
  overallReadiness: number;
  processes: ProcessStatus[];
  summary: {
    total: number;
    ready: number;
    pending: number;
  };
}

const PROCESS_DISPLAY_NAMES: { [key: string]: string } = {
  process_1_date_anchor: 'Date Anchor',
  process_2_destination: 'Destination',
  process_3_transportation: 'Transportation',
  process_4_accommodation: 'Accommodation',
  process_5_daily_itinerary: 'Daily Itinerary',
};

const MILESTONE_ORDER: { [key: string]: string[] } = {
  process_1_date_anchor: ['confirmed'],
  process_2_destination: ['confirmed'],
  process_3_transportation: ['researched', 'selected', 'booked'],
  process_4_accommodation: [
    'zone_researched',
    'zone_selected',
    'hotel_researched',
    'hotel_selected',
    'hotel_booked',
  ],
  process_5_daily_itinerary: ['researched', 'selected', 'confirmed'],
};

function loadTravelPlan(filePath: string): any {
  const content = fs.readFileSync(filePath, 'utf-8');
  return JSON.parse(content);
}

function evaluateProcess(
  processKey: string,
  data: any,
  rules: any
): ProcessStatus {
  const processData = data[processKey];
  const processRules = rules[processKey];

  if (!processRules) {
    return {
      name: processKey,
      displayName: PROCESS_DISPLAY_NAMES[processKey] || processKey,
      status: processData?.status || 'pending',
      readyToProceed: false,
      currentMilestone: null,
      nextMilestone: null,
      milestones: {},
      missingFields: [`No rules defined for ${processKey}`],
    };
  }

  // Evaluate ready_to_proceed
  const readyResult = evaluateRuleBlock(
    processRules.ready_to_proceed as RuleBlock,
    data
  );

  // Evaluate all milestones
  const milestoneResults: { [key: string]: boolean } = {};
  const milestonesMissing: { [key: string]: string[] } = {};

  if (processRules.milestones) {
    for (const [milestoneName, milestoneRules] of Object.entries(
      processRules.milestones
    )) {
      const result = evaluateRuleBlock(milestoneRules as RuleBlock, data);
      milestoneResults[milestoneName] = result.passed;
      milestonesMissing[milestoneName] = result.missingFields;
    }
  }

  // Determine current and next milestone
  const milestoneOrder = MILESTONE_ORDER[processKey] || [];
  let currentMilestone: string | null = null;
  let nextMilestone: string | null = null;

  for (let i = 0; i < milestoneOrder.length; i++) {
    const milestone = milestoneOrder[i];
    if (milestoneResults[milestone]) {
      currentMilestone = milestone;
    } else if (nextMilestone === null) {
      nextMilestone = milestone;
    }
  }

  // Focus missingFields on ready_to_proceed + next milestone only
  const actionableMissing: string[] = [...readyResult.missingFields];
  if (nextMilestone && milestonesMissing[nextMilestone]) {
    actionableMissing.push(...milestonesMissing[nextMilestone]);
  }

  return {
    name: processKey,
    displayName: PROCESS_DISPLAY_NAMES[processKey] || processKey,
    status: processData?.status || 'pending',
    readyToProceed: readyResult.passed,
    currentMilestone,
    nextMilestone,
    milestones: milestoneResults,
    missingFields: [...new Set(actionableMissing)], // deduplicate
  };
}

function calculateOverallReadiness(processes: ProcessStatus[]): number {
  const weights: { [key: string]: number } = {
    process_1_date_anchor: 10,
    process_2_destination: 10,
    process_3_transportation: 25,
    process_4_accommodation: 25,
    process_5_daily_itinerary: 30,
  };

  let totalWeight = 0;
  let achievedWeight = 0;

  for (const process of processes) {
    const weight = weights[process.name] || 20;
    totalWeight += weight;

    // Calculate process completion based on milestones
    const milestoneOrder = MILESTONE_ORDER[process.name] || [];
    if (milestoneOrder.length > 0) {
      let lastAchieved = -1;
      for (let i = 0; i < milestoneOrder.length; i++) {
        if (process.milestones[milestoneOrder[i]]) {
          lastAchieved = i;
        }
      }
      const progress = (lastAchieved + 1) / milestoneOrder.length;
      achievedWeight += weight * progress;
    } else if (process.readyToProceed) {
      achievedWeight += weight;
    }
  }

  return Math.round((achievedWeight / totalWeight) * 100);
}

function generateReport(data: any): StatusReport {
  const processKeys = [
    'process_1_date_anchor',
    'process_2_destination',
    'process_3_transportation',
    'process_4_accommodation',
    'process_5_daily_itinerary',
  ];

  const processes: ProcessStatus[] = [];

  for (const key of processKeys) {
    const status = evaluateProcess(key, data, data.readiness_rules);
    processes.push(status);
  }

  const readyCount = processes.filter((p) => p.readyToProceed).length;

  return {
    project: data.project,
    generatedAt: new Date().toISOString(),
    overallReadiness: calculateOverallReadiness(processes),
    processes,
    summary: {
      total: processes.length,
      ready: readyCount,
      pending: processes.length - readyCount,
    },
  };
}

function formatReport(report: StatusReport, ascii: boolean = false): string {
  const lines: string[] = [];

  const chars = ascii
    ? { hDouble: '=', hSingle: '-', filled: '#', empty: '.', check: '[x]', circle: '[ ]', bullet: '*', arrow: '->' }
    : { hDouble: '═', hSingle: '─', filled: '█', empty: '░', check: '✓', circle: '○', bullet: '●', arrow: '→' };

  lines.push(chars.hDouble.repeat(60));
  lines.push(`  TRAVEL PROJECT STATUS: ${report.project}`);
  lines.push(`  Generated: ${report.generatedAt}`);
  lines.push(chars.hDouble.repeat(60));
  lines.push('');

  // Overall readiness bar
  const barWidth = 40;
  const filledWidth = Math.round((report.overallReadiness / 100) * barWidth);
  const bar = chars.filled.repeat(filledWidth) + chars.empty.repeat(barWidth - filledWidth);
  lines.push(`  Overall Readiness: [${bar}] ${report.overallReadiness}%`);
  lines.push('');

  lines.push(chars.hSingle.repeat(60));
  lines.push('  PROCESS STATUS');
  lines.push(chars.hSingle.repeat(60));

  for (const process of report.processes) {
    const readyIcon = process.readyToProceed ? chars.check : chars.circle;
    const statusLabel = process.readyToProceed ? 'READY' : 'PENDING';

    lines.push('');
    lines.push(`  ${readyIcon} ${process.displayName}`);
    lines.push(`    Status: ${process.status} | Ready: ${statusLabel}`);

    if (process.currentMilestone) {
      lines.push(`    Current Milestone: ${process.currentMilestone}`);
    }
    if (process.nextMilestone) {
      lines.push(`    Next Milestone: ${process.nextMilestone}`);
    }

    // Show milestone progress
    const milestoneOrder = MILESTONE_ORDER[process.name] || [];
    if (milestoneOrder.length > 0) {
      const milestoneBar = milestoneOrder
        .map((m) => (process.milestones[m] ? chars.bullet : chars.circle))
        .join(` ${chars.arrow} `);
      lines.push(`    Milestones: ${milestoneBar}`);
      lines.push(`                ${milestoneOrder.join(` ${chars.arrow} `)}`);
    }

    // Show actionable missing fields (ready_to_proceed + next milestone only)
    if (process.missingFields.length > 0 && !process.readyToProceed) {
      lines.push(`    Actionable (${process.missingFields.length}):`);
      const toShow = process.missingFields.slice(0, 3);
      for (const field of toShow) {
        lines.push(`      - ${field}`);
      }
      if (process.missingFields.length > 3) {
        lines.push(
          `      ... and ${process.missingFields.length - 3} more`
        );
      }
    }
  }

  lines.push('');
  lines.push(chars.hSingle.repeat(60));
  lines.push('  SUMMARY');
  lines.push(chars.hSingle.repeat(60));
  lines.push(`  Processes Ready: ${report.summary.ready}/${report.summary.total}`);
  lines.push(`  Processes Pending: ${report.summary.pending}/${report.summary.total}`);
  lines.push(chars.hDouble.repeat(60));

  return lines.join('\n');
}

function printUsage(): void {
  console.error('Usage: npm run status -- [options]');
  console.error('       ts-node src/status/status-check.ts [options]');
  console.error('');
  console.error('Options:');
  console.error(`  --file <path>   Path to travel-plan.json (default: ${PATHS.defaultPlan})`);
  console.error('  --json          Output as JSON');
  console.error('  --ascii         Use ASCII-safe characters');
}

function parseArgs(args: string[]): { file: string | null; json: boolean; ascii: boolean } {
  let file: string | null = null;
  let json = false;
  let ascii = false;

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--file') {
      if (!args[i + 1] || args[i + 1].startsWith('--')) {
        console.error('Error: --file requires a path argument');
        printUsage();
        process.exit(1);
      }
      file = args[i + 1];
      i++;
    } else if (args[i] === '--json') {
      json = true;
    } else if (args[i] === '--ascii') {
      ascii = true;
    } else if (args[i] === '--help' || args[i] === '-h') {
      printUsage();
      process.exit(0);
    }
  }

  return { file, json, ascii };
}

function resolveDataPath(fileArg: string | null): string {
  if (fileArg) {
    // If absolute, use as-is; otherwise resolve from cwd
    return path.isAbsolute(fileArg)
      ? fileArg
      : path.resolve(process.cwd(), fileArg);
  }
  // Default: look for default plan in cwd
  return path.resolve(process.cwd(), PATHS.defaultPlan);
}

// Main execution
function main() {
  const args = process.argv.slice(2);
  const { file, json, ascii } = parseArgs(args);

  const dataPath = resolveDataPath(file);

  if (!fs.existsSync(dataPath)) {
    console.error(`Error: travel-plan.json not found at ${dataPath}`);
    printUsage();
    process.exit(1);
  }

  const data = loadTravelPlan(dataPath);
  const report = generateReport(data);

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(formatReport(report, ascii));
  }
}

main();
