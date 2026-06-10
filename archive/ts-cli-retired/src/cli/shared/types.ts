import type { StateManager } from '../../state/state-manager';

export interface ParsedArgs {
  /** Positional arguments after removing flags/options */
  cleanArgs: string[];
  /** Raw argv (including options) */
  raw: string[];
  /** Get a single option value by name (e.g., '--dest') */
  optionValue(name: string): string | undefined;
  /** Get all values for a repeated option (e.g., '--candidate') */
  optionValues(name: string): string[];
  /** Check if a boolean flag is present */
  hasFlag(flag: string): boolean;
}

export interface CliContext {
  sm: StateManager;
  args: ParsedArgs;
  dryRun: boolean;
  verbose: boolean;
  planId?: string;
}

export interface CommandHandler {
  /** One or more names this handler responds to (first is canonical) */
  names: string[];
  /** Short description for help text */
  description: string;
  /** Usage string for help text */
  usage: string;
  /** Whether this command needs a loaded StateManager (default: true) */
  requiresState?: boolean;
  /** Execute the command */
  execute(ctx: CliContext): Promise<void>;
}
