import { describe, it, expect } from 'vitest';

describe('tour-group-service', () => {
  it('exports the public API', async () => {
    const svc = await import('../../src/services/tour-group-service');
    expect(typeof svc.insertTourGroupOffers).toBe('function');
    expect(typeof svc.upsertScrapeAttempt).toBe('function');
    expect(typeof svc.listTourGroupOffers).toBe('function');
    expect(typeof svc.findAuditSet).toBe('function');
  });
});
