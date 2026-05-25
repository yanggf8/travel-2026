import type { ParsedArgs } from './types';

const OPTIONS_WITH_VALUES = new Set([
  '--dest', '--pax', '--plan-id', '--plan', '--state',
  '--goals', '--pace', '--assign', '--ref', '--book-by',
  '--selected', '--candidate', '--start', '--end', '--fixed',
  '--severity', '--types', '--source', '--flights', '--hotel-per-night',
  '--nights', '--package', '--max-price', '--fresh-hours', '--max',
  '--max-age', '--trip-id', '--category', '--status', '--limit',
  '--dir', '--files', '--note', '--flight', '--airline', '--airline-code',
  '--from', '--dep-terminal', '--dep', '--to', '--arr-terminal', '--arr',
  '--date', '--booked-date', '--name', '--check-in', '--access',
  '--duration', '--notes', '--start-time', '--zh', '--transit-zh',
  '--activities-zh-json', '--travel-date', '--travel-start', '--travel-end',
  '--run', '--file', '--origin', '--rate', '--dest-region',
]);

export function optionValue(args: string[], name: string): string | undefined {
  const eq = args.find(a => a.startsWith(`${name}=`));
  if (eq) return eq.slice(name.length + 1);
  const idx = args.indexOf(name);
  if (idx !== -1 && idx + 1 < args.length) return args[idx + 1];
  return undefined;
}

export function optionValues(args: string[], name: string): string[] {
  const values: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith(`${name}=`)) {
      values.push(a.slice(name.length + 1));
      continue;
    }
    if (a === name && i + 1 < args.length) {
      values.push(args[i + 1]);
      i++;
    }
  }
  return values;
}

export function makeParsedArgs(raw: string[]): ParsedArgs {
  const cleanArgs: string[] = [];
  for (let i = 0; i < raw.length; i++) {
    const a = raw[i];
    if (a.startsWith('--')) {
      if (OPTIONS_WITH_VALUES.has(a)) i++;
      continue;
    }
    cleanArgs.push(a);
  }

  return {
    cleanArgs,
    raw,
    optionValue: (name: string) => optionValue(raw, name),
    optionValues: (name: string) => optionValues(raw, name),
    hasFlag: (flag: string) => raw.includes(flag),
  };
}
