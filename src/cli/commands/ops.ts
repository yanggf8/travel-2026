import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

const runStatusCommand: CommandHandler = {
  names: ['run-status'],
  description: 'Show details for a specific operation run',
  usage: 'run-status [run-id]',
  async execute(ctx: CliContext): Promise<void> {
    const runId = ctx.args.cleanArgs[1];
    const planId = ctx.sm.getPlanId();
    const { getOperationRun } = require('../../services/turso-service');

    const run = await getOperationRun(planId, runId || undefined);
    if (!run) {
      console.log(runId ? `No operation found with run_id: ${runId}` : 'No operations found for this plan.');
      return;
    }
    console.log('\n📋 Operation Run Details');
    console.log('─'.repeat(50));
    console.log(`  Run ID:       ${run.run_id}`);
    console.log(`  Plan ID:      ${run.plan_id}`);
    console.log(`  Command:      ${run.command_type}`);
    if (run.command_summary) console.log(`  Summary:      ${run.command_summary}`);
    console.log(`  Status:       ${run.status === 'completed' ? '✅' : run.status === 'failed' ? '❌' : '⏳'} ${run.status}`);
    console.log(`  Version:      ${run.version_before}${run.version_after != null ? ` → ${run.version_after}` : ''}`);
    console.log(`  Started:      ${run.started_at}`);
    if (run.completed_at) console.log(`  Completed:    ${run.completed_at}`);
    if (run.error_message) console.log(`  Error:        ${run.error_message}`);
    console.log('');
  },
};

const runListCommand: CommandHandler = {
  names: ['run-list'],
  description: 'List recent operation runs for this plan',
  usage: 'run-list [--status completed|failed|started] [--limit N]',
  async execute(ctx: CliContext): Promise<void> {
    const planId = ctx.sm.getPlanId();
    const limitRaw = parseInt(ctx.args.optionValue('--limit') || '20', 10);
    const statusFilterOpt = ctx.args.optionValue('--status');
    const { listOperationRuns } = require('../../services/turso-service');

    const runs = await listOperationRuns(planId, {
      status: statusFilterOpt || undefined,
      limit: limitRaw || 20,
    });

    if (runs.length === 0) {
      console.log('No operations found.');
      return;
    }

    console.log(`\n📋 Recent Operations (${runs.length} results)`);
    console.log('─'.repeat(90));
    console.log(
      [
        'Status'.padEnd(5),
        'Command'.padEnd(22),
        'Summary'.padEnd(30),
        'Ver'.padEnd(7),
        'Started'.padEnd(20),
      ].join(' │ ')
    );
    console.log('─'.repeat(90));

    for (const run of runs) {
      const icon = run.status === 'completed' ? '✅' : run.status === 'failed' ? '❌' : '⏳';
      const ver = run.version_after != null
        ? `${run.version_before}→${run.version_after}`
        : `${run.version_before}`;
      const started = run.started_at ? run.started_at.slice(0, 19) : '-';

      console.log(
        [
          icon.padEnd(5),
          (run.command_type || '-').padEnd(22),
          (run.command_summary || '-').slice(0, 30).padEnd(30),
          ver.padEnd(7),
          started.padEnd(20),
        ].join(' │ ')
      );
    }
    console.log('─'.repeat(90));
    console.log('');
  },
};

registerCommand(runStatusCommand);
registerCommand(runListCommand);
