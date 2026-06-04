import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

// All three commands are pre-plan (requiresState: false) — Shaping Stage runs
// before any plan exists, so they must skip plan resolution.

function newRunId(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `shaping-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-` +
    `${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

// ── shaping-init ──────────────────────────────────────────────────────
// Creates an immutable research run. Destinations: --dest CODE:LABEL (repeatable).
// Durations: --nights N (repeatable). Window: --start / --end.
// Shaping (research directives + constraints) via repeatable --shaping flags.
// Format: --shaping ASPECT:ROLE:KIND:VALUE[:NOTES]
// ROLE examples: hard_constraint, soft_preference, search_directive
// Common ASPECT: date, channel, mobility, lodging, budget, activity, general

function parseShapingEntry(raw: string) {
  const parts = raw.split(':');
  if (parts.length < 4) {
    throw new Error(`--shaping must be ASPECT:ROLE:KIND:VALUE[:NOTES], got: "${raw}"`);
  }
  const [aspect, role, kind, value, ...notesParts] = parts;
  const notes = notesParts.length > 0 ? notesParts.join(':') : undefined;

  let value_text: string | undefined;
  let value_date: string | undefined;
  let value_integer: number | undefined;

  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    value_date = value;
  } else if (/^-?\d+$/.test(value)) {
    value_integer = parseInt(value, 10);
  } else {
    value_text = value;
  }

  return {
    aspect: aspect.toLowerCase(),
    role: role.toLowerCase(),
    kind: kind.toLowerCase(),
    value_text,
    value_date,
    value_integer,
    notes: notes || undefined,
  };
}

const shapingInitCommand: CommandHandler = {
  names: ['shaping-init'],
  description: 'Create a Shaping Stage research run (immutable inputs).',
  usage: 'shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 ' +
    '--dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 ' +
    '[--pax 2] [--rate 32] [--shaping ...] (repeatable)',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { createResearchRun } = require('../../services/shaping-service');
    const { args } = ctx;

    const origin = args.optionValue('--origin');
    const start = args.optionValue('--start');
    const end = args.optionValue('--end');
    const destOpts = args.optionValues('--dest');
    const nightsOpts = args.optionValues('--nights');
    const pax = parseInt(args.optionValue('--pax') || '2', 10);
    const rate = parseFloat(args.optionValue('--rate') || '32');
    const shapingOpts = args.optionValues('--shaping');

    if (!origin || !start || !end || destOpts.length === 0 || nightsOpts.length === 0) {
      console.error('Error: shaping-init requires --origin, --start, --end, ' +
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

    let shaping: any[] = [];
    if (shapingOpts.length > 0) {
      try {
        shaping = shapingOpts.map(parseShapingEntry);
      } catch (e: any) {
        console.error('Error parsing --shaping:', e.message);
        process.exit(1);
      }
    }

    const runId = newRunId();
    await createResearchRun({
      runId, originCode: origin.toUpperCase(), pax,
      windowStart: start, windowEnd: end, exchangeRateUsdTwd: rate,
      destinations, durations,
      shaping: shaping.length > 0 ? shaping : undefined,
    });

    console.log(`\n✅ Shaping Stage research run created: ${runId}`);
    console.log(`   Origin: ${origin.toUpperCase()}  Window: ${start} → ${end}  Pax: ${pax}`);
    console.log(`   Destinations: ${destinations.map((d) => d.destCode).join(', ')}`);
    console.log(`   Durations: ${durations.map((d) => d.nights + 'n').join(', ')}`);

    if (shaping.length > 0) {
      console.log(`\n   Shaping rules recorded (${shaping.length}):`);
      for (const s of shaping) {
        const val = s.value_date ?? s.value_text ?? (s.value_integer != null ? s.value_integer : '');
        const note = s.notes ? `  // ${s.notes}` : '';
        console.log(`     ${s.aspect}/${s.role}/${s.kind} = ${val}${note}`);
      }
    } else {
      // Shaping is the point of this stage; an empty run almost always means the
      // shaping step was skipped. Runs are immutable, so this becomes an orphan.
      console.error('\n⚠️  No --shaping rules recorded for this run.');
      console.error('   The Shaping Stage exists to capture constraints/preferences BEFORE research.');
      console.error('   Did you (1) load prior shaping from the DB and (2) record the new rules?');
      console.error('   Runs are immutable — if you skipped shaping, create a new run with --shaping');
      console.error('   ASPECT:ROLE:KIND:VALUE[:NOTES] (e.g. date:hard_constraint:return_no_later_than:2026-06-24).');
      console.error('   Check for prior shaping: db:exec "SELECT run_id FROM shaping_research_runs ORDER BY run_id DESC;"');
    }

    console.log(`\nNext: python scripts/shaping_research.py --run ${runId}`);
  },
};

// ── shaping-compare ───────────────────────────────────────────────────

const shapingCompareCommand: CommandHandler = {
  names: ['shaping-compare'],
  description: 'Show ranked Shaping Stage candidates across destinations.',
  usage: 'shaping-compare --run <run_id> [--json] [--limit N]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { getResearchRun, getCandidates } = require('../../services/shaping-service');
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: shaping-compare requires --run <run_id>');
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

    console.log(`\nShaping Stage Research — ${run.run_id}  (${run.origin_code}, ` +
      `${run.pax} pax, window ${run.window_start}..${run.window_end})`);

    if (run.shaping && run.shaping.length > 0) {
      console.log('\nResearch Shaping:');
      const byAspect: Record<string, typeof run.shaping> = {};
      for (const s of run.shaping) {
        (byAspect[s.aspect] ||= []).push(s);
      }
      for (const [aspect, items] of Object.entries(byAspect)) {
        console.log(`  ${aspect}:`);
        for (const s of items) {
          const val = s.value_date ?? s.value_text ?? (s.value_integer != null ? s.value_integer : '');
          const roleNote = s.role === 'hard_constraint' ? ' [HARD]' : (s.role === 'soft_preference' ? ' [PREF]' : '');
          console.log(`    - ${s.role}${roleNote} ${s.kind}${val !== '' ? ' = ' + val : ''}${s.notes ? '  // ' + s.notes : ''}`);
        }
      }
    }

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

// ── shaping-adopt ─────────────────────────────────────────────────────

const shapingAdoptCommand: CommandHandler = {
  names: ['shaping-adopt'],
  description: 'Record a Shaping Stage candidate as adopted into a plan.',
  usage: 'shaping-adopt <candidate_id> <plan_id> [--create-plan --dest <slug>]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { adoptCandidate, adoptCandidateToNewPlan } = require('../../services/shaping-service');
    const [, candidateId, planId] = ctx.args.cleanArgs;
    if (!candidateId || !planId) {
      console.error('Error: shaping-adopt requires <candidate_id> <plan_id>');
      process.exit(1);
    }
    if (ctx.args.hasFlag('--create-plan')) {
      const destinationSlug = ctx.args.optionValue('--dest');
      if (!destinationSlug) {
        console.error('Error: shaping-adopt --create-plan requires --dest <destination_slug>');
        process.exit(1);
      }
      await adoptCandidateToNewPlan({ candidateId, planId, destinationSlug });
      console.log(`✅ Candidate ${candidateId} adopted into new plan ${planId}`);
      console.log(`   Destination: ${destinationSlug}`);
      console.log('   P1 dates and P2 destination are seeded from the Shaping Stage candidate.');
      console.log(`   Next: npm run travel -- scaffold-itinerary --plan-id ${planId} --dest ${destinationSlug}`);
      return;
    }
    await adoptCandidate(candidateId, planId);
    console.log(`✅ Candidate ${candidateId} adopted into plan ${planId}`);
    console.log('   Next: set the locked dates/destination via /p1-dates and /p2-destination');
  },
};

// ── shaping-export ────────────────────────────────────────────────────
// Emits a run (run + destinations + durations + scrape attempts) as JSON.
// The Python aggregator consumes this instead of building SQL itself.

const shapingExportCommand: CommandHandler = {
  names: ['shaping-export'],
  description: 'Export a Shaping Stage research run as JSON (for the aggregator).',
  usage: 'shaping-export --run <run_id> --json',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { getResearchRun, getScrapeAttempts } = require('../../services/shaping-service');
    const runId = ctx.args.optionValue('--run');
    if (!runId) {
      console.error('Error: shaping-export requires --run <run_id>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const attempts = await getScrapeAttempts(runId);
    const shaping = await require('../../services/shaping-service').getResearchShaping(runId);
    // Single JSON object on stdout — nothing else, so Python can parse it.
    console.log(JSON.stringify({ ...run, attempts, shaping }));
  },
};

// ── shaping-import ────────────────────────────────────────────────────
// Consumes the Python aggregator's JSON handoff. Idempotent per pair:
// for each scraped (dest, nights) pair it first deletes any prior
// candidates for that pair, so a re-import never collides on the
// candidate_id PK. Inserts candidates with leave-days computed via the TS
// calculator, records scrape attempts, ranks, sets final run status.

const shapingImportCommand: CommandHandler = {
  names: ['shaping-import'],
  description: 'Import Shaping Stage aggregator results from a handoff JSON file.',
  usage: 'shaping-import --run <run_id> --file <path>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const fs = require('fs');
    const {
      insertCandidate, upsertScrapeAttempt, rankRun, setRunStatus,
      getResearchRun, getCandidates, deleteCandidatesForPair,
    } = require('../../services/shaping-service');
    const { calculateLeave } = require('../../utils/holiday-calculator');
    const { args } = ctx;

    const runId = args.optionValue('--run');
    const file = args.optionValue('--file');
    if (!runId || !file) {
      console.error('Error: shaping-import requires --run <run_id> and --file <path>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const payload = JSON.parse(fs.readFileSync(file, 'utf-8'));
    const candidates: any[] = payload.candidates || [];
    const attempts: any[] = payload.attempts || [];

    // Idempotent per pair: clear prior candidates for every pair THIS handoff
    // processed before inserting. The delete set is built from `attempts`,
    // not `candidates` — attempts are the authoritative list of pairs this
    // handoff scraped. A pair that scraped successfully but returned zero
    // flights still has an attempt row; building the set from `candidates`
    // would skip it and leave stale rows from a prior import.
    const pairs = new Set<string>();
    for (const a of attempts) pairs.add(`${a.destCode}|${a.nights}`);
    for (const key of pairs) {
      const [destCode, nightsStr] = key.split('|');
      await deleteCandidatesForPair(runId, destCode, parseInt(nightsStr, 10));
    }

    for (const a of attempts) {
      await upsertScrapeAttempt({
        runId, destCode: a.destCode, nights: a.nights,
        status: a.status, candidateCount: a.candidateCount, error: a.error,
      });
    }

    for (const c of candidates) {
      // Leave-days computed here — TS owns the holiday calendar.
      const leave = calculateLeave({
        startDate: c.departDate, endDate: c.returnDate, market: 'taiwan',
      });
      await insertCandidate({
        candidateId: c.candidateId, runId, destCode: c.destCode,
        departDate: c.departDate, returnDate: c.returnDate, nights: c.nights,
        flightTotalTwd: c.flightTotalTwd ?? null,
        leaveDays: leave.leaveDaysNeeded,
        verdict: c.verdict ?? null,
        flights: c.flights || [],
      });
    }

    // Rank if the run has any candidates at all (this import may have added
    // to candidates from an earlier partial run). Mark failed only when the
    // run is still completely empty.
    const allCandidates = await getCandidates(runId);
    if (allCandidates.length === 0) {
      await setRunStatus(runId, 'failed');
      console.log(`⚠️  No candidates for ${runId} — run marked failed.`);
      return;
    }
    await rankRun(runId);
    console.log(`✅ Imported ${candidates.length} candidates for ${runId} ` +
      `(${allCandidates.length} total), ranked.`);
    console.log(`   View: npm run travel -- shaping-compare --run ${runId}`);
  },
};

registerCommand(shapingInitCommand);
registerCommand(shapingCompareCommand);
registerCommand(shapingAdoptCommand);
registerCommand(shapingExportCommand);
registerCommand(shapingImportCommand);
