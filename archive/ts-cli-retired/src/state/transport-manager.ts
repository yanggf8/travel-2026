/**
 * Transport Manager
 * 
 * Handles P3 transportation operations:
 * - Airport transfers (arrival/departure)
 * - Ground transport candidates
 * - Transfer selection
 */

import type { TravelPlanMinimal, ProcessId } from './types';

export interface TransportOption {
  id: string;
  title: string;
  type: string;
  cost?: number;
  duration_min?: number;
  notes?: string;
}

export interface TransportSegment {
  status: 'planned' | 'booked' | 'confirmed';
  selected: TransportOption | null;
  candidates: TransportOption[];
}

export class TransportManager {
  constructor(
    private plan: TravelPlanMinimal,
    private timestamp: () => string,
    private emitEvent: (event: any) => void
  ) {}

  setAirportTransferSegment(
    destination: string,
    direction: 'arrival' | 'departure',
    segment: TransportSegment
  ): void {
    const p3 = this.ensureTransportationProcess(destination);

    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      p3.airport_transfers = {};
    }

    (p3.airport_transfers as Record<string, unknown>)[direction] = segment as unknown as Record<string, unknown>;
    this.touchTransportation(destination);

    this.emitEvent({
      event: 'airport_transfer_updated',
      destination,
      process: 'process_3_transportation',
      data: {
        direction,
        status: segment.status,
        selected_id: segment.selected?.id ?? null,
        candidates_count: segment.candidates?.length ?? 0,
      },
    });
  }

  addAirportTransferCandidate(
    destination: string,
    direction: 'arrival' | 'departure',
    option: TransportOption
  ): void {
    const p3 = this.ensureTransportationProcess(destination);
    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      p3.airport_transfers = {};
    }

    const transfers = p3.airport_transfers as Record<string, unknown>;
    const existing = (transfers[direction] as Record<string, unknown> | undefined) ?? {
      status: 'planned',
      selected: null,
      candidates: [],
    };

    const candidates = (existing.candidates as TransportOption[] | undefined) ?? [];
    if (!candidates.some(c => c.id === option.id)) {
      candidates.push(option);
    }
    existing.candidates = candidates;
    transfers[direction] = existing;

    this.touchTransportation(destination);
    this.emitEvent({
      event: 'airport_transfer_candidate_added',
      destination,
      process: 'process_3_transportation',
      data: { direction, option_id: option.id, title: option.title },
    });
  }

  selectAirportTransferOption(
    destination: string,
    direction: 'arrival' | 'departure',
    optionId: string
  ): void {
    const p3 = this.ensureTransportationProcess(destination);
    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      throw new Error(`No airport transfers set for ${destination}`);
    }

    const transfers = p3.airport_transfers as Record<string, unknown>;
    const segment = transfers[direction] as Record<string, unknown> | undefined;
    if (!segment) {
      throw new Error(`No ${direction} airport transfer segment found`);
    }

    const candidates = (segment.candidates as TransportOption[] | undefined) ?? [];
    const selected = candidates.find(c => c.id === optionId) as TransportOption | undefined;
    if (!selected) {
      throw new Error(`Airport transfer option not found: ${optionId}`);
    }

    segment.selected = selected;
    transfers[direction] = segment;

    this.touchTransportation(destination);
    this.emitEvent({
      event: 'airport_transfer_selected',
      destination,
      process: 'process_3_transportation',
      data: { direction, option_id: optionId, title: selected.title },
    });
  }

  private ensureTransportationProcess(destination: string): Record<string, unknown> {
    const destObj = this.plan.destinations[destination];
    if (!destObj) {
      throw new Error(`Destination not found: ${destination}`);
    }

    if (!destObj.process_3_transportation) {
      (destObj as Record<string, unknown>).process_3_transportation = {
        status: 'pending',
        updated_at: this.timestamp(),
      };
    }

    const p3 = destObj.process_3_transportation as Record<string, unknown>;
    if (typeof p3.status !== 'string') {
      p3.status = 'pending';
    }
    return p3;
  }

  private touchTransportation(destination: string): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    if (p3) {
      p3.updated_at = this.timestamp();
    }
  }
}
