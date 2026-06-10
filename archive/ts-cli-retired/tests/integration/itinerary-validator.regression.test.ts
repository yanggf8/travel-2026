import { describe, expect, it } from 'vitest';
import { ItineraryValidator } from '../../src/validation/itinerary-validator';
import type { DaySummary } from '../../src/validation/types';

function dayWithPendingBooking(date: string): DaySummary {
  return {
    dayNumber: 1,
    date,
    theme: 'Museum day',
    activities: [
      {
        id: 'teamlab',
        title: 'teamLab Borderless',
        day: 1,
        session: 'morning',
        durationMin: 120,
        isFixedTime: false,
        bookingRequired: true,
        bookingStatus: 'pending',
        bookByDate: '2026-02-10',
      },
    ],
    areas: [],
    totalDurationMin: 120,
    fixedTimeCount: 0,
    pendingBookings: 1,
  };
}

describe('ItineraryValidator booking deadlines', () => {
  it('does not fail historical itinerary days for past booking deadlines', () => {
    const validator = new ItineraryValidator();
    const result = validator.validate(
      [dayWithPendingBooking('2026-02-13')],
      new Date('2026-05-22T00:00:00'),
    );

    expect(result.issues.filter((issue) => issue.category === 'booking_deadline')).toEqual([]);
    expect(result.valid).toBe(true);
  });

  it('still fails active/future itinerary days for passed booking deadlines', () => {
    const validator = new ItineraryValidator();
    const result = validator.validate(
      [dayWithPendingBooking('2026-05-22')],
      new Date('2026-05-22T00:00:00'),
    );

    expect(result.issues).toContainEqual(
      expect.objectContaining({
        severity: 'error',
        category: 'booking_deadline',
        activityId: 'teamlab',
      }),
    );
    expect(result.valid).toBe(false);
  });
});
