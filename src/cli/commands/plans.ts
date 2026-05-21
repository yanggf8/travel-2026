import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatPlanList, listPlans, todayIso } from '../shared/plan-resolver';

const plansCommand: CommandHandler = {
  names: ['plans', 'list-plans'],
  description: 'List travel plans and their date anchors.',
  usage: 'plans [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const plans = await listPlans();
    if (ctx.args.hasFlag('--json')) {
      console.log(JSON.stringify(plans, null, 2));
      return;
    }
    console.log(formatPlanList(plans, todayIso()));
  },
};

registerCommand(plansCommand);
