import { validatePositiveInt } from '../../types/validation';
import type { StateManager } from '../../state/state-manager';

const VALID_SESSIONS = ['morning', 'noon', 'afternoon', 'evening'] as const;

export function requireDayArg(dayStr: string | undefined, label = '<day>'): number {
  if (!dayStr) {
    console.error(`Error: ${label} is required`);
    process.exit(1);
  }
  const result = validatePositiveInt(dayStr, label);
  if (!result.ok) {
    console.error(`Error: ${result.error}`);
    process.exit(1);
  }
  return result.value;
}

export function requireSessionArg(session: string | undefined): string {
  if (!session || !VALID_SESSIONS.includes(session as any)) {
    console.error('Error: <session> must be one of: morning | noon | afternoon | evening');
    process.exit(1);
  }
  return session;
}

export function resolveDest(sm: StateManager, destOpt: string | undefined): string {
  return sm.resolveDestination(destOpt);
}
