import type { CommandHandler } from '../shared/types';

const handlers = new Map<string, CommandHandler>();

export function registerCommand(handler: CommandHandler): void {
  for (const name of handler.names) {
    handlers.set(name, handler);
  }
}

export function getCommandHandler(name: string): CommandHandler | undefined {
  return handlers.get(name);
}

export function getAllCommands(): CommandHandler[] {
  const seen = new Set<string>();
  const result: CommandHandler[] = [];
  for (const h of handlers.values()) {
    if (!seen.has(h.names[0])) {
      seen.add(h.names[0]);
      result.push(h);
    }
  }
  return result;
}
