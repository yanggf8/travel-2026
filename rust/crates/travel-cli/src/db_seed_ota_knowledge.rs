//! `db seed ota-knowledge` — port of scripts/seed-ota-knowledge.ts.
//!
//! Seeds OTA domain knowledge into Turso. Data is inlined as typed Rust constants
//! (curated domain knowledge, not fetched), so no JSON file is read at runtime.
//! Idempotent via INSERT OR REPLACE.
//!
//! Tables: airlines, booking_types, booking_type_rules, platform_behaviors,
//! platform_behavior_quirks, platform_behavior_baggage_labels, comparison_rules.
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

use crate::db::connect_write;
use libsql::params;

const SOURCE_URL: &str = "repo:src/skills/travel-shared/references/ota-knowledge.json#v1.0.0";
const CONFIDENCE: &str = "curated";

const FSC_BAG: &str = "Included (typically 23-30kg)";
const FSC_MEAL: &str = "Included";
const LCC_BAG: &str = "NOT included (purchase 20kg: ~TWD 1,500-2,500/direction/person). When booked through travel agency FIT, 20kg baggage IS included.";
const LCC_MEAL: &str = "NOT included";

// ── data types ───────────────────────────────────────────────────────────────

struct Airline {
    code: &'static str,
    name: &'static str,
    type_: &'static str,
    hand_baggage_kg: i64,
    checked_bag_included: bool,
    checked_bag_kg: i64,
    checked_bag_cost_twd: Option<i64>,
    checked_bag_cost_jpy: Option<i64>,
}

struct BookingType {
    slug: &'static str,
    name_zh: &'static str,
    description: &'static str,
    rules: &'static [&'static str],
}

struct PlatformBehavior {
    platform: &'static str,
    currency: &'static str,
    price_display: &'static str,
    baggage_labels: &'static [(&'static str, &'static str)],
    quirks: &'static [&'static str],
}

// ── data ─────────────────────────────────────────────────────────────────────

fn airlines() -> &'static [Airline] {
    &[
        Airline { code: "MM", name: "Peach", type_: "LCC", hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: Some(550), checked_bag_cost_jpy: Some(2500) },
        Airline { code: "SL", name: "Thai Lion Air", type_: "LCC", hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: Some(650), checked_bag_cost_jpy: Some(3000) },
        Airline { code: "IT", name: "Tigerair Taiwan", type_: "LCC", hand_baggage_kg: 10, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: Some(600), checked_bag_cost_jpy: Some(2700) },
        Airline { code: "GK", name: "Jetstar Japan", type_: "LCC", hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: Some(580), checked_bag_cost_jpy: Some(2600) },
        Airline { code: "VZ", name: "Thai Vietjet Air", type_: "LCC", hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: Some(700), checked_bag_cost_jpy: Some(3200) },
        Airline { code: "BR", name: "EVA Air", type_: "FSC", hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: None, checked_bag_cost_jpy: None },
        Airline { code: "CI", name: "China Airlines", type_: "FSC", hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: None, checked_bag_cost_jpy: None },
        Airline { code: "JX", name: "STARLUX Airlines", type_: "FSC", hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: None, checked_bag_cost_jpy: None },
    ]
}

fn booking_types() -> &'static [BookingType] {
    &[
        BookingType {
            slug: "fit_package", name_zh: "FIT 自由配",
            description: "Travel agency bundled flight + hotel, traveler plans own itinerary",
            rules: &[
                "Checked baggage (typically 20kg) is ALWAYS included, even for LCC flights",
                "Price is per person unless explicitly stated otherwise",
                "Travel agency provides after-sales support (rebooking, cancellation)",
                "Package may include airport transfers or other extras",
            ],
        },
        BookingType {
            slug: "group_tour", name_zh: "團體旅遊",
            description: "Fully guided group tour with fixed itinerary",
            rules: &[
                "All-inclusive: flight, hotel, meals, transport, guide",
                "Fixed departure dates with minimum group size",
                "Price is per person",
            ],
        },
        BookingType {
            slug: "separate_booking", name_zh: "分開訂",
            description: "Book flight and hotel independently",
            rules: &[
                "LCC flights typically do NOT include checked baggage",
                "Full-service airlines (EVA, China Airlines, Cathay, JAL, ANA) include checked baggage",
                "Hotel prices may be per room per night, not per person",
                "No single point of contact for support",
            ],
        },
    ]
}

fn platform_behaviors() -> &'static [PlatformBehavior] {
    &[
        PlatformBehavior {
            platform: "trip_com", currency: "USD",
            price_display: "per person for flights, total shown separately",
            baggage_labels: &[
                ("Included", "Checked baggage included in fare"),
                ("Carry-on baggage included", "Only carry-on, NO checked baggage - must purchase separately"),
            ],
            quirks: &[
                "Hotel detail pages require login",
                "Roundtrip search only shows outbound flights",
                "Scrape return as separate one-way search",
                "Prices in USD - convert using EXCHANGE_RATES.USD_TWD",
            ],
        },
        PlatformBehavior {
            platform: "booking_com", currency: "TWD",
            price_display: "per room per night",
            baggage_labels: &[],
            quirks: &[
                "Cloudflare bot protection - may need retry",
                "Prices shown are 'starting from' and may not reflect actual dates",
                "Taxes and service fees may be separate",
                "Use lang=zh-tw for Traditional Chinese",
            ],
        },
        PlatformBehavior {
            platform: "liontravel", currency: "TWD",
            price_display: "per person for FIT, total shown in search results",
            baggage_labels: &[],
            quirks: &[
                "FIT search via vacation.liontravel.com",
                "Group tour URL format unknown",
                "Thursday FITPKG promo: TWD 400 off (min TWD 20,000)",
                "Date-specific pricing varies dramatically",
            ],
        },
    ]
}

fn comparison_rules() -> &'static [&'static str] {
    &[
        "Always compare same date range across FIT and separate booking",
        "For LCC separate booking: add baggage fee (2 pax × 2 directions × ~TWD 1,750)",
        "For FIT with LCC: baggage is included, do NOT add extra fee",
        "Use leave-calculator with holiday calendar for fair leave day comparison",
        "Convert all prices to TWD before comparing",
        "Hotel comparison: match similar class (e.g. Just Sleep ≈ mid-range business hotel)",
    ]
}

// ── run ──────────────────────────────────────────────────────────────────────

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;

    let conn = connect_write().await?;
    let fetched_at = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // airlines
    for a in airlines() {
        let fsc = a.type_ == "FSC";
        let (bag_note, meal_note) = if fsc { (FSC_BAG, FSC_MEAL) } else { (LCC_BAG, LCC_MEAL) };
        conn.execute(
            "INSERT OR REPLACE INTO airlines (code, name, type, hand_baggage_kg, checked_bag_included, checked_bag_kg, checked_bag_cost_twd, checked_bag_cost_jpy, classification_baggage_note, classification_meal, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![a.code, a.name, a.type_, a.hand_baggage_kg, a.checked_bag_included as i64,
                a.checked_bag_kg, a.checked_bag_cost_twd, a.checked_bag_cost_jpy,
                bag_note, meal_note, SOURCE_URL, fetched_at.clone(), CONFIDENCE],
        ).await.map_err(|e| format!("airlines insert failed for {}: {e}", a.code))?;
    }

    // booking_types + rules
    for b in booking_types() {
        conn.execute(
            "INSERT OR REPLACE INTO booking_types (slug, name_zh, description, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![b.slug, b.name_zh, b.description, SOURCE_URL, fetched_at.clone(), CONFIDENCE],
        ).await.map_err(|e| format!("booking_types insert failed for {}: {e}", b.slug))?;

        conn.execute("DELETE FROM booking_type_rules WHERE booking_type = ?1", params![b.slug])
            .await.map_err(|e| format!("delete booking_type_rules failed: {e}"))?;
        for (i, rule) in b.rules.iter().enumerate() {
            conn.execute(
                "INSERT INTO booking_type_rules (booking_type, sort_order, rule) VALUES (?1, ?2, ?3)",
                params![b.slug, i as i64, *rule],
            ).await.map_err(|e| format!("booking_type_rules insert failed: {e}"))?;
        }
    }

    // platform_behaviors + quirks + baggage_labels
    for p in platform_behaviors() {
        conn.execute(
            "INSERT OR REPLACE INTO platform_behaviors (platform, currency, price_display, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p.platform, p.currency, p.price_display, SOURCE_URL, fetched_at.clone(), CONFIDENCE],
        ).await.map_err(|e| format!("platform_behaviors insert failed for {}: {e}", p.platform))?;

        conn.execute("DELETE FROM platform_behavior_quirks WHERE platform = ?1", params![p.platform])
            .await.map_err(|e| format!("delete platform_behavior_quirks failed: {e}"))?;
        for (i, quirk) in p.quirks.iter().enumerate() {
            conn.execute(
                "INSERT INTO platform_behavior_quirks (platform, sort_order, quirk) VALUES (?1, ?2, ?3)",
                params![p.platform, i as i64, *quirk],
            ).await.map_err(|e| format!("platform_behavior_quirks insert failed: {e}"))?;
        }

        conn.execute("DELETE FROM platform_behavior_baggage_labels WHERE platform = ?1", params![p.platform])
            .await.map_err(|e| format!("delete platform_behavior_baggage_labels failed: {e}"))?;
        for (label, desc) in p.baggage_labels {
            conn.execute(
                "INSERT INTO platform_behavior_baggage_labels (platform, label, description) VALUES (?1, ?2, ?3)",
                params![p.platform, *label, *desc],
            ).await.map_err(|e| format!("platform_behavior_baggage_labels insert failed: {e}"))?;
        }
    }

    // comparison_rules
    for (i, rule) in comparison_rules().iter().enumerate() {
        let idx = (i + 1) as i64;
        conn.execute(
            "INSERT OR REPLACE INTO comparison_rules (id, rule, sort_order, source_url, fetched_at, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![idx, *rule, idx, SOURCE_URL, fetched_at.clone(), CONFIDENCE],
        ).await.map_err(|e| format!("comparison_rules insert failed: {e}"))?;
    }

    let na = airlines().len();
    let nb = booking_types().len();
    let np = platform_behaviors().len();
    let nc = comparison_rules().len();
    println!("Seeded: {na} airlines, {nb} booking_types, {np} platform_behaviors, {nc} comparison_rules.");
    Ok(())
}
