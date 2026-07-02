//! Shaping purchase-matrix reads — run header, rules, flight candidates, and package offers.
//! All keyed on `run_id` with a bound `?1` param. Pure data; scoring lives in the CLI command.

use libsql::Connection;

#[derive(Debug, Clone)]
pub struct RunHeader {
    pub pax: i64,
    pub currency: String,
    pub origin_code: String,
}

/// `(pax, currency, origin_code)` from `shaping_research_runs`; `None` if the run is unknown.
pub async fn run_header(
    conn: &Connection,
    run_id: &str,
) -> Result<Option<RunHeader>, String> {
    let mut rows = conn
        .query(
            "SELECT pax, currency, origin_code FROM shaping_research_runs WHERE run_id = ?1",
            libsql::params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("failed to query shaping_research_runs: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read shaping_research_runs: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some(RunHeader {
        pax: r.get::<i64>(0).unwrap_or(0),
        currency: r.get::<String>(1).unwrap_or_default(),
        origin_code: r.get::<String>(2).unwrap_or_default(),
    }))
}

#[derive(Debug, Clone)]
pub struct RuleRow {
    pub aspect: String,
    pub role: String,
    pub kind: String,
    pub value_text: Option<String>,
    pub value_date: Option<String>,
    pub value_integer: Option<i64>,
}

pub async fn rules(conn: &Connection, run_id: &str) -> Result<Vec<RuleRow>, String> {
    let mut rows = conn
        .query(
            "SELECT aspect, role, kind, value_text, value_date, value_integer \
             FROM shaping_rules WHERE run_id = ?1",
            libsql::params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("failed to query shaping_rules: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read shaping_rules row: {e}"))?
    {
        out.push(RuleRow {
            aspect: r.get::<String>(0).unwrap_or_default(),
            role: r.get::<String>(1).unwrap_or_default(),
            kind: r.get::<String>(2).unwrap_or_default(),
            value_text: r.get::<Option<String>>(3).unwrap_or(None),
            value_date: r.get::<Option<String>>(4).unwrap_or(None),
            value_integer: r.get::<Option<i64>>(5).unwrap_or(None),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CandidateRow {
    pub candidate_id: String,
    pub depart_date: String,
    pub return_date: String,
    pub nights: i64,
    pub flight_total_twd: Option<i64>,
    pub leave_days: Option<i64>,
    pub rank: Option<i64>,
    pub verdict: Option<String>,
}

pub async fn candidates(conn: &Connection, run_id: &str) -> Result<Vec<CandidateRow>, String> {
    let mut rows = conn
        .query(
            "SELECT candidate_id, depart_date, return_date, nights, flight_total_twd, \
             leave_days, rank, verdict FROM shaping_candidates WHERE run_id = ?1",
            libsql::params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("failed to query shaping_candidates: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read shaping_candidates row: {e}"))?
    {
        out.push(CandidateRow {
            candidate_id: r.get::<String>(0).unwrap_or_default(),
            depart_date: r.get::<String>(1).unwrap_or_default(),
            return_date: r.get::<String>(2).unwrap_or_default(),
            nights: r.get::<i64>(3).unwrap_or(0),
            flight_total_twd: r.get::<Option<i64>>(4).unwrap_or(None),
            leave_days: r.get::<Option<i64>>(5).unwrap_or(None),
            rank: r.get::<Option<i64>>(6).unwrap_or(None),
            verdict: r.get::<Option<String>>(7).unwrap_or(None),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct OfferRow {
    pub offer_id: String,
    pub source_id: String,
    pub depart_date: String,
    pub return_date: String,
    pub nights: i64,
    pub price_per_person_twd: i64,
    pub title: String,
    pub hotel_name: Option<String>,
    pub hotel_star_rating: Option<i64>,
    pub meals_included_count: Option<i64>,
    pub departure_status: Option<String>,
    pub seats_available: Option<i64>,
}

pub async fn offers(conn: &Connection, run_id: &str) -> Result<Vec<OfferRow>, String> {
    let mut rows = conn
        .query(
            "SELECT offer_id, source_id, depart_date, return_date, nights, price_per_person_twd, \
             title, hotel_name, hotel_star_rating, meals_included_count, departure_status, \
             seats_available FROM shaping_tour_group_offers WHERE run_id = ?1",
            libsql::params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("failed to query shaping_tour_group_offers: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read shaping_tour_group_offers row: {e}"))?
    {
        out.push(OfferRow {
            offer_id: r.get::<String>(0).unwrap_or_default(),
            source_id: r.get::<String>(1).unwrap_or_default(),
            depart_date: r.get::<String>(2).unwrap_or_default(),
            return_date: r.get::<String>(3).unwrap_or_default(),
            nights: r.get::<i64>(4).unwrap_or(0),
            price_per_person_twd: r.get::<i64>(5).unwrap_or(0),
            title: r.get::<String>(6).unwrap_or_default(),
            hotel_name: r.get::<Option<String>>(7).unwrap_or(None),
            hotel_star_rating: r.get::<Option<i64>>(8).unwrap_or(None),
            meals_included_count: r.get::<Option<i64>>(9).unwrap_or(None),
            departure_status: r.get::<Option<String>>(10).unwrap_or(None),
            seats_available: r.get::<Option<i64>>(11).unwrap_or(None),
        });
    }
    Ok(out)
}