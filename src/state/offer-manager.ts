/**
 * Offer Manager
 * 
 * Handles P3+4 package offer operations:
 * - Import offers from scrapers
 * - Update availability/pricing
 * - Select offers for booking
 * - Populate cascade (P3/P4 from offer)
 */

import type { TravelPlanMinimal, ProcessId } from './types';

export class OfferManager {
  constructor(
    private plan: TravelPlanMinimal,
    private timestamp: () => string,
    private emitEvent: (event: any) => void,
    private setProcessStatus: (dest: string, process: ProcessId, status: any) => void,
    private clearDirty: (dest: string, process: ProcessId) => void
  ) {}

  updateOfferAvailability(
    destination: string,
    offerId: string,
    date: string,
    availability: 'available' | 'sold_out' | 'limited',
    price?: number,
    seatsRemaining?: number,
    source: string = 'user'
  ): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) throw new Error(`Destination not found: ${destination}`);

    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    const offers = results?.offers as Array<Record<string, unknown>> | undefined;
    if (!offers) throw new Error(`No offers found in ${destination}.process_3_4_packages.results`);

    const offer = offers.find(o => o.id === offerId);
    if (!offer) throw new Error(`Offer not found: ${offerId}`);

    let datePricing = offer.date_pricing as Record<string, Record<string, unknown>> | undefined;
    if (!datePricing) {
      datePricing = {};
      offer.date_pricing = datePricing;
    }

    const previousEntry = datePricing[date];
    const previousAvailability = previousEntry?.availability;

    datePricing[date] = {
      ...previousEntry,
      availability,
      ...(price !== undefined && { price }),
      ...(seatsRemaining !== undefined && { seats_remaining: seatsRemaining }),
      note: `Updated by ${source} at ${this.timestamp()}`,
    };

    this.emitEvent({
      event: 'offer_availability_updated',
      destination,
      process: 'process_3_4_packages',
      data: { offer_id: offerId, date, from: previousAvailability, to: availability, price, seats_remaining: seatsRemaining, source },
    });
  }

  selectOffer(destination: string, offerId: string, date: string, populateCascade: boolean = true): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) throw new Error(`Destination not found: ${destination}`);

    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    const offers = results?.offers as Array<Record<string, unknown>> | undefined;
    if (!offers) throw new Error(`No offers found`);

    const offer = offers.find(o => o.id === offerId);
    if (!offer) throw new Error(`Offer not found: ${offerId}`);

    if (!packages) throw new Error('Packages process not found');
    packages.selected_offer_id = offerId;
    packages.chosen_offer = { id: offerId, selected_date: date, selected_at: this.timestamp() };
    if (!packages.results || typeof packages.results !== 'object') packages.results = {};
    (packages.results as Record<string, unknown>).chosen_offer = offer;

    this.setProcessStatus(destination, 'process_3_4_packages', 'selected');

    this.emitEvent({
      event: 'offer_selected',
      destination,
      process: 'process_3_4_packages',
      data: {
        offer_id: offerId,
        date,
        offer_name: offer.name,
        hotel: (offer.hotel as Record<string, unknown>)?.name,
        price_total: (offer.date_pricing as Record<string, Record<string, unknown>>)?.[date]?.price,
      },
    });

    if (populateCascade) {
      this.populateFromOffer(destination, offer, date);
    }
  }

  private populateFromOffer(destination: string, offer: Record<string, unknown>, date: string): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    const flight = offer.flight as Record<string, unknown> | undefined;
    if (flight) {
      const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
      if (p3) {
        p3.populated_from = `package:${offer.id}`;
        p3.flight = { ...flight, booked_date: date, populated_at: this.timestamp() };
        this.setProcessStatus(destination, 'process_3_transportation', 'populated');
        this.clearDirty(destination, 'process_3_transportation');
      }
    }

    const hotel = offer.hotel as Record<string, unknown> | undefined;
    if (hotel) {
      const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
      if (p4) {
        p4.populated_from = `package:${offer.id}`;
        p4.hotel = { ...hotel, check_in: date, populated_at: this.timestamp() };
        this.setProcessStatus(destination, 'process_4_accommodation', 'populated');
        this.clearDirty(destination, 'process_4_accommodation');
      }
    }

    this.emitEvent({
      event: 'cascade_populated',
      destination,
      data: { source: `package:${offer.id}`, populated: ['process_3_transportation', 'process_4_accommodation'] },
    });
  }

  importPackageOffers(
    destination: string,
    sourceId: string,
    offers: Array<Record<string, unknown>>,
    note?: string,
    warnings?: string[]
  ): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) throw new Error(`Destination not found: ${destination}`);

    if (!destObj.process_3_4_packages) {
      (destObj as Record<string, unknown>).process_3_4_packages = {};
    }

    const p34 = destObj.process_3_4_packages as Record<string, unknown>;
    if (!p34.results || typeof p34.results !== 'object') p34.results = {};
    const results = p34.results as Record<string, unknown>;

    results.offers = offers;
    const provenance = (results.provenance as Array<Record<string, unknown>> | undefined) ?? [];
    provenance.push({
      source_id: sourceId,
      scraped_at: this.timestamp(),
      offers_found: offers.length,
      ...(note ? { note } : {}),
    });
    results.provenance = provenance;

    if (warnings && warnings.length > 0) {
      const existing = (results.warnings as string[] | undefined) ?? [];
      results.warnings = [...existing, ...warnings];
    }

    const currentStatus = this.getProcessStatus(destination, 'process_3_4_packages');
    if (!currentStatus || currentStatus === 'pending' || currentStatus === 'researching') {
      this.setProcessStatus(destination, 'process_3_4_packages', 'researched');
    } else {
      p34.updated_at = this.timestamp();
    }

    this.emitEvent({
      event: 'package_offers_imported',
      destination,
      process: 'process_3_4_packages',
      data: { source_id: sourceId, offers_found: offers.length, note },
    });
  }

  private getProcessStatus(destination: string, process: ProcessId): string | null {
    const dest = this.plan.destinations[destination];
    if (!dest || !dest[process]) return null;
    const processObj = dest[process] as Record<string, unknown>;
    return (processObj['status'] as string) || null;
  }
}
