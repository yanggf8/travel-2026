export function allocateClustersToDays(
  clusterIds: string[],
  days: Array<Record<string, unknown>>,
  explicitAssignments?: Map<string, number>
): Array<{ clusterId: string; dayNumber: number }> {
  const fullDays = days.filter(d => d.day_type === 'full').map(d => d.day_number as number);
  const arrivalDay = days.find(d => d.day_type === 'arrival')?.day_number as number | undefined;
  const departureDay = days.find(d => d.day_type === 'departure')?.day_number as number | undefined;
  const dayNumbers = new Set(days.map(d => d.day_number as number));

  const assignedDays = new Set(explicitAssignments?.values() || []);
  const availableFullDays = fullDays.filter(d => !assignedDays.has(d));

  const result: Array<{ clusterId: string; dayNumber: number }> = [];
  let fullIdx = 0;

  for (const id of clusterIds) {
    let target: number | undefined;
    const explicit = explicitAssignments?.get(id);
    if (explicit && dayNumbers.has(explicit)) target = explicit;
    if (id.includes('last_day') && departureDay) target = departureDay;
    if (!target) {
      target = availableFullDays.length ? availableFullDays[fullIdx % availableFullDays.length] : undefined;
      fullIdx++;
    }
    if (!target && arrivalDay) target = arrivalDay;
    if (!target && departureDay) target = departureDay;
    if (!target) target = 1;

    result.push({ clusterId: id, dayNumber: target });
  }

  return result;
}

export function parseAssignments(assignOpt: string | undefined): Map<string, number> | undefined {
  if (!assignOpt) return undefined;
  const map = new Map<string, number>();
  for (const part of assignOpt.split(',')) {
    const [clusterId, dayStr] = part.split(':').map(s => s.trim());
    if (!clusterId || !dayStr) continue;
    const day = parseInt(dayStr, 10);
    if (!Number.isFinite(day) || day <= 0) continue;
    map.set(clusterId, day);
  }
  return map.size ? map : undefined;
}

export function getSessionOrderForDayType(dayType: string): Array<'morning' | 'noon' | 'afternoon' | 'evening'> {
  if (dayType === 'arrival') return ['afternoon', 'evening'];
  if (dayType === 'departure') return ['morning', 'noon'];
  return ['morning', 'noon', 'afternoon', 'evening'];
}

export function chunkEvenly<T>(items: T[], buckets: number): T[][] {
  const out: T[][] = Array.from({ length: buckets }, () => []);
  for (let i = 0; i < items.length; i++) {
    out[i % buckets].push(items[i]);
  }
  return out;
}
