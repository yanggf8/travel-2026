import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

// All three commands are pre-plan (requiresState: false) — Stage 0 runs
// before any plan exists, so they must skip plan resolution.

function newRunId(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `stage0-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-` +
    `${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

// ── stage0-init ──────────────────────────────────────────────────────
// Creates an immutable research run. Destinations: --dest CODE:LABEL (repeatable).
// Durations: --nights N (repeatable). Window: --start / --end.

const stage0InitCommand: CommandHandler = {
  names: ['stage0-init'],
  description: 'Create a Stage 0 research run (immutable inputs).',
  usage: 'stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 ' +
    '--dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 ' +
    '[--pax 2] [--rate 32]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { createResearchRun } = require('../../services/stage0-service');
    const { args } = ctx;

    const origin = args.optionValue('--origin');
    const start = args.optionValue('--start');
    const end = args.optionValue('--end');
    const destOpts = args.optionValues('--dest');
    const nightsOpts = args.optionValues('--nights');
    const pax = parseInt(args.optionValue('--pax') || '2', 10);
    const rate = parseFloat(args.optionValue('--rate') || '32');

    if (!origin || !start || !end || destOpts.length === 0 || nightsOpts.length === 0) {
      console.error('Error: stage0-init requires --origin, --start, --end, ' +
        'at least one --dest CODE:LABEL, and at least one --nights N');
      process.exit(1);
    }

    const destinations = destOpts.map((d) => {
      const idx = d.indexOf(':');
      if (idx === -1) {
        console.error(`Error: --dest must be CODE:LABEL (got: ${d})`);
        process.exit(1);
      }
      return { destCode: d.slice(0, idx).toUpperCase(), destLabel: d.slice(idx + 1) };
    });
    const durations = nightsOpts.map((n) => ({ nights: parseInt(n, 10) }));

    const runId = newRunId();
    await createResearchRun({
      runId, originCode: origin.toUpperCase(), pax,
      windowStart: start, windowEnd: end, exchangeRateUsdTwd: rate,
      destinations, durations,
    });

    console.log(`\n✅ Stage 0 research run created: ${runId}`);
    console.log(`   Origin: ${origin.toUpperCase()}  Window: ${start} → ${end}  Pax: ${pax}`);
    console.log(`   Destinations: ${destinations.map((d) => d.destCode).join(', ')}`);
    console.log(`   Durations: ${durations.map((d) => d.nights + 'n').join(', ')}`);
    console.log(`\nNext: python scripts/stage0_research.py --run ${runId}`);
  },
};

// ── stage0-compare ───────────────────────────────────────────────────

const stage0CompareCommand: CommandHandler = {
  names: ['stage0-compare'],
  description: 'Show ranked Stage 0 candidates across destinations.',
  usage: 'stage0-compare --run <run_id> [--json] [--limit N]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { getResearchRun, getCandidates } = require('../../services/stage0-service');
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: stage0-compare requires --run <run_id>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const limit = parseInt(args.optionValue('--limit') || '0', 10);
    let candidates = await getCandidates(runId);
    if (limit > 0) candidates = candidates.slice(0, limit);

    if (args.hasFlag('--json')) {
      console.log(JSON.stringify({ run, candidates }, null, 2));
      return;
    }

    console.log(`\nStage 0 Research — ${run.run_id}  (${run.origin_code}, ` +
      `${run.pax} pax, window ${run.window_start}..${run.window_end})`);
    console.log('');
    if (candidates.length === 0) {
      console.log('(no candidates — run the aggregator first)\n');
      return;
    }
    const header = [
      '#'.padEnd(3), 'Dest'.padEnd(5), 'Depart'.padEnd(12), 'Return'.padEnd(12),
      'Nights'.padEnd(7), 'Flight (party)'.padEnd(16), 'Leave'.padEnd(6), 'Verdict',
    ].join(' ');
    console.log(header);
    console.log('─'.repeat(header.length));
    for (const c of candidates) {
      const price = c.flight_total_twd == null
        ? 'n/a' : `${run.currency} ${c.flight_total_twd.toLocaleString()}`;
      console.log([
        String(c.rank ?? '-').padEnd(3),
        c.dest_code.padEnd(5),
        c.depart_date.padEnd(12),
        c.return_date.padEnd(12),
        `${c.nights}n`.padEnd(7),
        price.padEnd(16),
        String(c.leave_days ?? '-').padEnd(6),
        c.verdict ?? '',
      ].join(' '));
    }
    console.log('');
  },
};

// ── stage0-adopt ─────────────────────────────────────────────────────

const stage0AdoptCommand: CommandHandler = {
  names: ['stage0-adopt'],
  description: 'Record a Stage 0 candidate as adopted into a plan.',
  usage: 'stage0-adopt <candidate_id> <plan_id>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { adoptCandidate } = require('../../services/stage0-service');
    const [, candidateId, planId] = ctx.args.cleanArgs;
    if (!candidateId || !planId) {
      console.error('Error: stage0-adopt requires <candidate_id> <plan_id>');
      process.exit(1);
    }
    await adoptCandidate(candidateId, planId);
    console.log(`✅ Candidate ${candidateId} adopted into plan ${planId}`);
    console.log('   Next: set the locked dates/destination via /p1-dates and /p2-destination');
  },
};

registerCommand(stage0InitCommand);
registerCommand(stage0CompareCommand);
registerCommand(stage0AdoptCommand);
