/**
 * Itinerary Manager
 * 
 * Handles P5 daily itinerary operations:
 * - Scaffold day structures
 * - Activity CRUD (add/update/remove)
 * - Booking status tracking
 * - Time management
 */

import type { TravelPlanMinimal, ProcessId } from './types';

export class ItineraryManager {
  constructor(
    private plan: TravelPlanMinimal,
    private timestamp: () => string,
    private emitEvent: (event: any) => void,
    private setProcessStatus: (dest: string, process: ProcessId, status: any) => void,
    private forceSetProcessStatus: (dest: string, process: ProcessId, status: any, data?: Record<string, unknown>) => void,
    private clearDirty: (dest: string, process: ProcessId) => void
  ) {}

  scaffoldItinerary(destination: string, days: Array<Record<string, unknown>>, force: boolean = false): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) throw new Error(`Destination not found: ${destination}`);

    if (!destObj.process_5_daily_itinerary) {
      (destObj as Record<string, unknown>).process_5_daily_itinerary = {};
    }

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown>;
    const currentStatus = this.getProcessStatus(destination, 'process_5_daily_itinerary');

    if (force && currentStatus && !['pending', 'researching'].includes(currentStatus)) {
      this.forceSetProcessStatus(destination, 'process_5_daily_itinerary', 'pending', {
        reason: 'force re-scaffold',
        from: currentStatus,
      });
    }

    p5.days = days;
    p5.updated_at = this.timestamp();
    p5.scaffolded_at = this.timestamp();

    this.setProcessStatus(destination, 'process_5_daily_itinerary', 'researching');
    this.clearDirty(destination, 'process_5_daily_itinerary');

    this.emitEvent({
      event: 'itinerary_scaffolded',
      destination,
      process: 'process_5_daily_itinerary',
      data: { days_count: days.length, day_types: days.map(d => d.day_type) },
    });
  }

  addActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activity: {
      title: string;
      area?: string;
      nearest_station?: string;
      duration_min?: number;
      booking_required?: boolean;
      booking_url?: string;
      cost_estimate?: number;
      tags?: string[];
      notes?: string;
      priority?: 'must' | 'want' | 'optional';
    }
  ): string {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as { activities: Array<Record<string, unknown>> };
    if (!sessionObj || !Array.isArray(sessionObj.activities)) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const id = `activity_${Date.now().toString(36)}_${Math.random().toString(36).substring(2, 6)}`;

    const fullActivity = {
      id,
      title: activity.title,
      area: activity.area || '',
      nearest_station: activity.nearest_station || null,
      duration_min: activity.duration_min || null,
      booking_required: activity.booking_required || false,
      booking_url: activity.booking_url || null,
      cost_estimate: activity.cost_estimate || null,
      tags: activity.tags || [],
      notes: activity.notes || null,
      priority: activity.priority || 'want',
    };

    sessionObj.activities.push(fullActivity);
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_added',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: id, title: activity.title },
    });

    return id;
  }

  updateActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityId: string,
    updates: Partial<Record<string, unknown>>
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityId);
    if (idx === -1) {
      throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);
    }

    const current = sessionObj.activities[idx];
    const activityObj = typeof current === 'string'
      ? this.upgradeStringActivity(current, { booking_required: false })
      : current;
    sessionObj.activities[idx] = activityObj;
    Object.assign(activityObj, updates);
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: activityObj.id, updates: Object.keys(updates) },
    });
  }

  removeActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityId: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityId);
    if (idx === -1) {
      throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);
    }

    const removed = sessionObj.activities.splice(idx, 1)[0];
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_removed',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: typeof removed === 'string' ? null : removed.id,
        title: typeof removed === 'string' ? removed : (removed.title as string | undefined),
      },
    });
  }

  setActivityBookingStatus(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityIdOrTitle: string,
    status: 'not_required' | 'pending' | 'booked' | 'waitlist',
    ref?: string,
    bookBy?: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const activityIdx = this.findActivityIndex(sessionObj.activities, activityIdOrTitle);
    if (activityIdx === -1) {
      throw new Error(`Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`);
    }

    const activity = sessionObj.activities[activityIdx];
    const wasUpgraded = typeof activity === 'string';
    const activityObj = wasUpgraded
      ? this.upgradeStringActivity(activity, { booking_required: true })
      : activity;
    if (wasUpgraded) {
      sessionObj.activities[activityIdx] = activityObj;
    }

    const previousStatus = activityObj.booking_status as string | undefined;
    activityObj.booking_status = status;

    if (ref !== undefined) activityObj.booking_ref = ref;
    if (bookBy !== undefined) activityObj.book_by = bookBy;

    if (status === 'booked' || status === 'pending' || status === 'waitlist') {
      activityObj.booking_required = true;
    }

    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_booking_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: activityObj.id,
        title: activityObj.title,
        from_status: previousStatus,
        to_status: status,
        booking_ref: ref,
        book_by: bookBy,
        upgraded_from_string: wasUpgraded,
      },
    });
  }

  setActivityTime(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityIdOrTitle: string,
    opts: { start_time?: string; end_time?: string; is_fixed_time?: boolean }
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityIdOrTitle);
    if (idx === -1) {
      throw new Error(`Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`);
    }

    const current = sessionObj.activities[idx];
    const activityObj = typeof current === 'string'
      ? this.upgradeStringActivity(current, { booking_required: false })
      : current;
    sessionObj.activities[idx] = activityObj;

    const previous = {
      start_time: activityObj.start_time as string | undefined,
      end_time: activityObj.end_time as string | undefined,
      is_fixed_time: activityObj.is_fixed_time as boolean | undefined,
    };

    if (opts.start_time !== undefined) activityObj.start_time = opts.start_time;
    if (opts.end_time !== undefined) activityObj.end_time = opts.end_time;
    if (opts.is_fixed_time !== undefined) activityObj.is_fixed_time = opts.is_fixed_time;

    this.touchItinerary(destination);
    this.emitEvent({
      event: 'activity_time_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: activityObj.id,
        title: activityObj.title,
        from: previous,
        to: {
          start_time: activityObj.start_time,
          end_time: activityObj.end_time,
          is_fixed_time: activityObj.is_fixed_time,
        },
      },
    });
  }

  setDayTheme(destination: string, dayNumber: number, theme: string | null): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    day.theme = theme;
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'itinerary_day_theme_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, theme },
    });
  }

  setSessionFocus(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    focus: string | null
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as Record<string, unknown> | undefined;
    if (!sessionObj) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    sessionObj.focus = focus;
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'itinerary_session_focus_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, focus },
    });
  }

  setSessionTimeRange(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    start: string,
    end: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const sessionObj = day[session] as Record<string, unknown> | undefined;
    if (!sessionObj) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    sessionObj.time_range = { start, end };
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'session_time_range_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, start, end },
    });
  }

  findActivity(
    destination: string,
    idOrTitle: string
  ): { dayNumber: number; session: 'morning' | 'afternoon' | 'evening'; activity: string | Record<string, unknown>; isString: boolean } | null {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    const sessions: Array<'morning' | 'afternoon' | 'evening'> = ['morning', 'afternoon', 'evening'];
    const searchLower = idOrTitle.toLowerCase();

    for (const day of days) {
      const dayNumber = day.day_number as number;
      for (const session of sessions) {
        const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> } | undefined;
        if (!sessionObj?.activities) continue;

        for (const a of sessionObj.activities) {
          if (typeof a === 'string') {
            if (a.toLowerCase().includes(searchLower)) {
              return { dayNumber, session, activity: a, isString: true };
            }
          } else {
            const id = a.id as string | undefined;
            const title = a.title as string | undefined;
            if (id === idOrTitle || (title && title.toLowerCase().includes(searchLower))) {
              return { dayNumber, session, activity: a, isString: false };
            }
          }
        }
      }
    }

    return null;
  }

  private getDay(destination: string, dayNumber: number): Record<string, unknown> | null {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    return days.find(d => d.day_number === dayNumber) || null;
  }

  private getProcessStatus(destination: string, process: ProcessId): string | null {
    const dest = this.plan.destinations[destination];
    if (!dest || !dest[process]) return null;
    const processObj = dest[process] as Record<string, unknown>;
    return (processObj['status'] as string) || null;
  }

  private touchItinerary(destination: string): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    if (p5) {
      p5.updated_at = this.timestamp();
    }
  }

  private upgradeStringActivity(title: string, overrides?: Partial<Record<string, unknown>>): Record<string, unknown> {
    const id = `activity_${Date.now().toString(36)}_${Math.random().toString(36).substring(2, 6)}`;
    return {
      id,
      title,
      area: '',
      nearest_station: null,
      duration_min: null,
      booking_required: false,
      booking_url: null,
      booking_status: undefined,
      booking_ref: undefined,
      book_by: undefined,
      cost_estimate: null,
      tags: [],
      notes: null,
      priority: 'want',
      ...(overrides || {}),
    };
  }

  private findActivityIndex(activities: Array<string | Record<string, unknown>>, idOrTitle: string): number {
    const idx = activities.findIndex(a => typeof a !== 'string' && a.id === idOrTitle);
    if (idx !== -1) return idx;

    const searchLower = idOrTitle.toLowerCase();
    return activities.findIndex(a => {
      if (typeof a === 'string') {
        return a.toLowerCase().includes(searchLower);
      }
      const title = a.title as string | undefined;
      return Boolean(title && title.toLowerCase().includes(searchLower));
    });
  }
}
