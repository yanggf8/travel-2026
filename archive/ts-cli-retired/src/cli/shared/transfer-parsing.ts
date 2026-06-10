import type { TransportOption } from '../../state/types';

export function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 48);
}

export function hashString(text: string): string {
  let hash = 5381;
  for (let i = 0; i < text.length; i++) {
    hash = ((hash << 5) + hash) + text.charCodeAt(i);
    hash |= 0;
  }
  return Math.abs(hash).toString(36);
}

export function parseTransferSpec(direction: 'arrival' | 'departure', spec: string): TransportOption {
  const parts = spec.split('|').map(p => p.trim());
  const title = parts[0];
  const route = parts[1];
  if (!title || !route) {
    throw new Error(`Invalid transfer spec (need at least title|route): "${spec}"`);
  }

  const durationMin = parts[2] ? parseInt(parts[2], 10) : undefined;
  const priceYen = parts[3] ? parseInt(parts[3], 10) : undefined;
  const schedule = parts[4] || undefined;

  const id = `${direction}_${slugify(title)}_${hashString(route).slice(0, 6)}`;

  return {
    id,
    title,
    route,
    ...(Number.isFinite(durationMin) ? { duration_min: durationMin } : {}),
    ...(Number.isFinite(priceYen) ? { price_yen: priceYen } : {}),
    ...(schedule ? { schedule } : {}),
  };
}
