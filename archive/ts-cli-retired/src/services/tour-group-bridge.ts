import { sqlInt, sqlText } from '../state/sql-helpers';
import {
  findAuditSet,
  getTursoClient,
  type TourGroupOfferRow,
} from './tour-group-service';

export interface BridgeInput {
  run_id: string;
  plan_id: string;
  destination: string;
  dest_region: string;
  nights: number;
  qualityFloorStarRating?: number;
}

/**
 * Copy the curated audit set for this (run, dest_region, nights) into
 * plan_offers + plan_offer_group_meta. Idempotent (INSERT OR REPLACE).
 *
 * Audit set: raw cheapest + quality-floor cheapest + top-3 per
 * (source_id, depart_date). See
 * docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md §5.
 */
export async function bridgeAuditSet(input: BridgeInput): Promise<TourGroupOfferRow[]> {
  const audit = await findAuditSet(
    input.run_id, input.dest_region, input.nights,
    { qualityFloorStarRating: input.qualityFloorStarRating }
  );
  if (audit.length === 0) return [];

  const client = getTursoClient();
  const stmts: string[] = [];
  for (const o of audit) {
    stmts.push(
      `INSERT OR REPLACE INTO plan_offers
        (plan_id, destination, id, source_id, type, title, price_per_person, currency,
         availability, url, scraped_at, duration_days, package_subtype)
       VALUES (${sqlText(input.plan_id)}, ${sqlText(input.destination)}, ${sqlText(o.offer_id)},
         ${sqlText(o.source_id)}, ${sqlText('package')}, ${sqlText(o.title)},
         ${sqlInt(o.price_per_person_twd)}, ${sqlText('TWD')},
         ${sqlText(o.departure_status ?? null)}, ${sqlText(o.url)}, ${sqlText(o.scraped_at)},
         ${sqlInt(o.nights + 1)}, ${sqlText('group_tour')});`
    );
    stmts.push(
      `INSERT OR REPLACE INTO plan_offer_group_meta
        (plan_id, destination, offer_id, meals_included_count, departure_status,
         seats_available, min_group_size, group_size_cap, source_offer_run_id, source_offer_id)
       VALUES (${sqlText(input.plan_id)}, ${sqlText(input.destination)}, ${sqlText(o.offer_id)},
         ${sqlInt(o.meals_included_count ?? null)}, ${sqlText(o.departure_status ?? null)},
         ${sqlInt(o.seats_available ?? null)}, ${sqlInt(o.min_group_size ?? null)},
         ${sqlInt(o.group_size_cap ?? null)}, ${sqlText(o.run_id)}, ${sqlText(o.offer_id)});`
    );
  }
  await client.executeMany(stmts);
  return audit;
}
