//! Adopt-time tour-group bridge — port of src/services/tour-group-bridge.ts
//! (`bridgeAuditSet`) + the `findAuditSet` selector in tour-group-service.ts.
//!
//! Copies the curated audit set for a (run_id, dest_region, nights) into
//! `plan_offers` + `plan_offer_group_meta` at adopt time. Idempotent
//! (INSERT OR REPLACE). Invoked from shaping-adopt --create-plan to match the
//! TS adoptCandidateToNewPlan flow (shaping-service.ts:573).
//!
//! Audit set = raw cheapest ∪ quality-floor cheapest (≥ floor stars) ∪ top-3
//! per (source_id, depart_date). Spec:
//! docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md §5.

use libsql::{params, Connection};
use std::collections::BTreeMap;
use travel_db::repo::plan_offers::{self, BridgeOfferWrite};

const DEFAULT_QUALITY_FLOOR: i64 = 4;

#[derive(Clone)]
pub struct AuditOffer {
    pub offer_id: String,
    pub source_id: String,
    pub depart_date: String,
    pub nights: i64,
    pub price_per_person_twd: i64,
    pub title: String,
    pub url: String,
    pub scraped_at: String,
    pub hotel_star_rating: Option<i64>,
    pub meals_included_count: Option<i64>,
    pub departure_status: Option<String>,
    pub seats_available: Option<i64>,
    pub min_group_size: Option<i64>,
    pub group_size_cap: Option<i64>,
}

/// listTourGroupOffers({ run_id, dest_region, nights }) ORDER BY price ASC.
async fn list_offers(
    conn: &Connection,
    run_id: &str,
    dest_region: &str,
    nights: i64,
) -> Result<Vec<AuditOffer>, String> {
    let mut rows = conn
        .query(
            "SELECT offer_id, source_id, depart_date, nights, price_per_person_twd, title, url, \
                    scraped_at, hotel_star_rating, meals_included_count, departure_status, \
                    seats_available, min_group_size, group_size_cap \
             FROM shaping_tour_group_offers \
             WHERE run_id = ?1 AND dest_region = ?2 AND nights = ?3 \
             ORDER BY price_per_person_twd ASC",
            params![run_id.to_string(), dest_region.to_string(), nights],
        )
        .await
        .map_err(|e| format!("shaping_tour_group_offers query failed: {e}"))?;

    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("tour-group offer row read failed: {e}"))?
    {
        out.push(AuditOffer {
            offer_id: r.get::<String>(0).unwrap_or_default(),
            source_id: r.get::<String>(1).unwrap_or_default(),
            depart_date: r.get::<String>(2).unwrap_or_default(),
            nights: r.get::<i64>(3).unwrap_or(0),
            price_per_person_twd: r.get::<i64>(4).unwrap_or(0),
            title: r.get::<String>(5).unwrap_or_default(),
            url: r.get::<String>(6).unwrap_or_default(),
            scraped_at: r.get::<String>(7).unwrap_or_default(),
            hotel_star_rating: r.get::<Option<i64>>(8).unwrap_or(None),
            meals_included_count: r.get::<Option<i64>>(9).unwrap_or(None),
            departure_status: r.get::<Option<String>>(10).unwrap_or(None),
            seats_available: r.get::<Option<i64>>(11).unwrap_or(None),
            min_group_size: r.get::<Option<i64>>(12).unwrap_or(None),
            group_size_cap: r.get::<Option<i64>>(13).unwrap_or(None),
        });
    }
    Ok(out)
}

/// findAuditSet: raw cheapest + quality-floor cheapest + top-3 per
/// (source_id, depart_date). `all` must already be sorted price ASC.
/// Returns the picked rows sorted price ASC (offer_id-deduped).
pub fn find_audit_set(all: &[AuditOffer], quality_floor: i64) -> Vec<AuditOffer> {
    if all.is_empty() {
        return Vec::new();
    }
    // offer_id -> row, insertion-independent (we sort at the end).
    let mut picked: BTreeMap<String, AuditOffer> = BTreeMap::new();

    // raw cheapest (all[0], already price ASC)
    picked.insert(all[0].offer_id.clone(), all[0].clone());

    // quality-floor cheapest (first row meeting the star floor)
    if let Some(qf) = all
        .iter()
        .find(|o| o.hotel_star_rating.unwrap_or(0) >= quality_floor)
    {
        picked.insert(qf.offer_id.clone(), qf.clone());
    }

    // top-3 per (source_id, depart_date)
    let mut groups: BTreeMap<String, Vec<AuditOffer>> = BTreeMap::new();
    for o in all {
        let key = format!("{}|{}", o.source_id, o.depart_date);
        groups.entry(key).or_default().push(o.clone());
    }
    for rows in groups.values_mut() {
        rows.sort_by_key(|o| o.price_per_person_twd);
        for r in rows.iter().take(3) {
            picked.insert(r.offer_id.clone(), r.clone());
        }
    }

    let mut result: Vec<AuditOffer> = picked.into_values().collect();
    result.sort_by_key(|o| o.price_per_person_twd);
    result
}

/// bridgeAuditSet: copy the audit set into plan_offers + plan_offer_group_meta.
/// Idempotent. Returns the number of offers bridged.
pub async fn bridge_audit_set(
    conn: &Connection,
    run_id: &str,
    plan_id: &str,
    destination: &str,
    dest_region: &str,
    nights: i64,
    quality_floor: Option<i64>,
) -> Result<usize, String> {
    let floor = quality_floor.unwrap_or(DEFAULT_QUALITY_FLOOR);
    let all = list_offers(conn, run_id, dest_region, nights).await?;
    let audit = find_audit_set(&all, floor);
    if audit.is_empty() {
        return Ok(0);
    }

    for o in &audit {
        let w = BridgeOfferWrite {
            plan_id: plan_id.to_string(),
            destination: destination.to_string(),
            offer_id: o.offer_id.clone(),
            source_id: o.source_id.clone(),
            title: o.title.clone(),
            price_per_person: o.price_per_person_twd,
            departure_status: o.departure_status.clone(),
            url: o.url.clone(),
            scraped_at: o.scraped_at.clone(),
            duration_days: o.nights + 1,
            meals_included_count: o.meals_included_count,
            seats_available: o.seats_available,
            min_group_size: o.min_group_size,
            group_size_cap: o.group_size_cap,
            source_run_id: run_id.to_string(),
        };
        plan_offers::insert_bridge_offer(conn, &w).await?;
    }

    Ok(audit.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(id: &str, source: &str, date: &str, price: i64, stars: Option<i64>) -> AuditOffer {
        AuditOffer {
            offer_id: id.to_string(),
            source_id: source.to_string(),
            depart_date: date.to_string(),
            nights: 5,
            price_per_person_twd: price,
            title: "t".to_string(),
            url: "u".to_string(),
            scraped_at: "2026-05-25T00:00:00Z".to_string(),
            hotel_star_rating: stars,
            meals_included_count: None,
            departure_status: None,
            seats_available: None,
            min_group_size: None,
            group_size_cap: None,
        }
    }

    fn ids(set: &[AuditOffer]) -> Vec<String> {
        set.iter().map(|x| x.offer_id.clone()).collect()
    }

    #[test]
    fn audit_set_raw_quality_top3() {
        // price ASC input (matches list_offers ordering)
        let all = vec![
            o("a1", "besttour", "2026-06-20", 28000, Some(3)), // raw cheapest + top-3 besttour
            o("a3", "besttour", "2026-06-20", 30000, Some(3)), // top-3 besttour
            o("b1", "lifetour", "2026-06-20", 31000, Some(3)), // top-3 lifetour
            o("a2", "besttour", "2026-06-20", 35000, Some(4)), // quality-floor + top-3 besttour
            o("a4", "besttour", "2026-06-20", 42000, Some(5)), // 4th besttour → excluded
        ];
        let picked = ids(&find_audit_set(&all, 4));
        assert!(picked.contains(&"a1".to_string()), "raw cheapest");
        assert!(picked.contains(&"a2".to_string()), "quality-floor");
        assert!(picked.contains(&"a3".to_string()), "top-3 besttour");
        assert!(picked.contains(&"b1".to_string()), "top-3 lifetour");
        assert!(!picked.contains(&"a4".to_string()), "4th besttour excluded");
    }

    #[test]
    fn audit_set_empty_quality_floor_ok() {
        // no 4-star rows → quality-floor contributes nothing, no panic
        let all = vec![
            o("a1", "besttour", "2026-06-20", 28000, Some(3)),
            o("a3", "besttour", "2026-06-20", 30000, Some(3)),
        ];
        let picked = ids(&find_audit_set(&all, 4));
        assert_eq!(picked, vec!["a1".to_string(), "a3".to_string()]);
    }

    #[test]
    fn audit_set_empty_input() {
        assert!(find_audit_set(&[], 4).is_empty());
    }
}
