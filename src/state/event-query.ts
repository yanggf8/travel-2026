/**
 * Event Query Helpers
 * 
 * Query utilities for StateManager event log
 */

import type { TravelEvent } from './types';

export class EventQuery {
  constructor(private getEvents: () => TravelEvent[]) {}

  getEventsByType(type: string): TravelEvent[] {
    return this.getEvents().filter(e => e.event === type);
  }

  getEventsForDestination(dest: string): TravelEvent[] {
    return this.getEvents().filter(e => e.destination === dest);
  }

  getRecentEvents(hours: number): TravelEvent[] {
    const cutoff = new Date(Date.now() - hours * 60 * 60 * 1000);
    return this.getEvents().filter(e => new Date(e.at) >= cutoff);
  }

  getEventsByProcess(dest: string, process: string): TravelEvent[] {
    return this.getEvents().filter(e => e.destination === dest && e.process === process);
  }
}
