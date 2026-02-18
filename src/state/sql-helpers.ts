/**
 * Shared SQL helpers for turso-repository and plan-repository.
 * Centralises rowsToObjects, rowsToObjectsAt, and SQL value formatters.
 */

/** Parse the first result in a single-query pipeline response. */
export function rowsToObjects(response: any): Record<string, any>[] {
  return rowsToObjectsAt(response, 0);
}

/** Parse the ith result in a multi-query batch pipeline response. */
export function rowsToObjectsAt(response: any, idx: number): Record<string, any>[] {
  const result = response?.results?.[idx]?.response?.result;
  if (!result?.rows || !result?.cols) return [];
  const cols = (result.cols as Array<{ name: string }>).map((c) => c.name);
  return (result.rows as unknown[][]).map((row) => {
    const obj: Record<string, any> = {};
    for (let i = 0; i < cols.length; i++) {
      const cell = row[i] as any;
      obj[cols[i]] = cell?.value ?? null;
    }
    return obj;
  });
}

export function sqlText(v: string | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return `'${String(v).replace(/'/g, "''")}'`;
}

export function sqlInt(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(Math.round(v));
}

export function sqlReal(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(v);
}

export function sqlBool(v: boolean | null | undefined): string {
  if (v === null || v === undefined) return '0';
  return v ? '1' : '0';
}
