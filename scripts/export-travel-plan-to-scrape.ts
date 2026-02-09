#!/usr/bin/env ts-node
import fs from 'node:fs';
import path from 'node:path';

const TRAVEL_PLAN_PATH = 'data/trips/osaka-kyoto-2026/travel-plan.json';
const OUTPUT_DIR = 'data/trips/osaka-kyoto-2026';

interface TravelPlan {
  destinations: Record<string, {
    slug: string;
    process_3_4_packages?: {
      results?: {
        offers?: Array<{
          id: string;
          source_id: string;
          product_code?: string;
          url: string;
          scraped_at: string;
          type: string;
          package_subtype?: string;
          currency: string;
          price_per_person: number;
          price_total?: number;
          availability: string;
          duration_days: number;
          flight?: {
            airline: string;
            outbound: { date: string; departure: string; arrival: string; from: string; to: string };
            return: { date: string; departure: string; arrival: string; from: string; to: string };
          };
          hotel?: {
            name: string;
            area?: string;
            nights: number;
          };
          includes?: string[];
        }>;
      };
    };
  }>;
}

function convertOfferToScrapeFormat(offer: any, destinationSlug: string) {
  return {
    id: offer.id,
    url: offer.url,
    title: offer.hotel?.name || offer.id,
    name: offer.hotel?.name || offer.id,
    source_id: offer.source_id,
    product_code: offer.product_code,
    scraped_at: new Date().toISOString(), // Use current timestamp for re-import
    type: offer.type,
    package_subtype: offer.package_subtype,
    currency: offer.currency,
    price_per_person: offer.price_per_person,
    availability: offer.availability,
    flight: offer.flight,
    hotel: offer.hotel,
    duration_days: offer.duration_days,
    includes: offer.includes,
    // Add best_value for date extraction by parser
    best_value: offer.flight?.outbound?.date ? {
      date: offer.flight.outbound.date,
      price: offer.price_per_person
    } : undefined,
  };
}

function main() {
  const planPath = path.resolve(TRAVEL_PLAN_PATH);
  if (!fs.existsSync(planPath)) {
    console.error(`❌ Travel plan not found: ${planPath}`);
    process.exit(1);
  }

  const plan: TravelPlan = JSON.parse(fs.readFileSync(planPath, 'utf-8'));
  const outputDir = path.resolve(OUTPUT_DIR);
  
  let totalExported = 0;

  for (const [destKey, dest] of Object.entries(plan.destinations)) {
    const offers = dest.process_3_4_packages?.results?.offers || [];
    if (offers.length === 0) continue;

    const scrapeData = {
      offers: offers.map(offer => convertOfferToScrapeFormat(offer, dest.slug))
    };
    const outputFile = path.join(outputDir, `${dest.slug}-packages-scrape.json`);
    
    fs.writeFileSync(outputFile, JSON.stringify(scrapeData, null, 2));
    console.log(`✅ Exported ${scrapeData.offers.length} offers from ${dest.slug} → ${outputFile}`);
    totalExported += scrapeData.offers.length;
  }

  console.log(`\n📦 Total: ${totalExported} offers exported`);
  console.log(`\n💡 Next step: npm run db:import:turso -- --files ${OUTPUT_DIR}/osaka_kyoto_2026-packages-scrape.json --destination osaka_kyoto_2026 --region kansai`);
}

main();
