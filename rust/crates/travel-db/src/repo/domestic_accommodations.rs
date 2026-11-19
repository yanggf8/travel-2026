//! Domestic accommodations — normalized table `domestic_accommodations` (no JSON).
//!
//! Slug-keyed reference data for Taiwan domestic stays (Phase A).
//! Read-only query with parameterized WHERE (OfferFilter pattern), no sql_quote.

use libsql::Connection;

/// The tax basis a published rate was recorded under (price_basis column).
/// `incl` = the price is the tax-and-fees-inclusive total (Booking.com shows
/// this); `excl` = pre-tax room rate, taxes/fees added at checkout (Agoda's
/// default). NULL = 未確認 — never guessed, never derived from `price_source`
/// text (no default data). Legal values live only in this enum (no DB CHECK,
/// project rule) — mirror of `records::RecordStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceBasis {
    Incl,
    Excl,
}

impl PriceBasis {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "incl" => Ok(PriceBasis::Incl),
            "excl" => Ok(PriceBasis::Excl),
            other => Err(format!(
                "invalid price basis \"{other}\" (legal values: incl | excl)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PriceBasis::Incl => "incl",
            PriceBasis::Excl => "excl",
        }
    }
}

/// One domestic accommodation row selected by the query.
#[derive(Debug, Clone)]
pub struct DomesticAccommodationRow {
    pub id: String,
    pub destination: String,
    pub hotel_name: String,
    pub room_type: String,
    pub sea_view: i64,
    pub max_occupancy: Option<i64>,
    pub price_twd: i64,
    pub currency: String,
    pub breakfast_included: i64,
    pub source: Option<String>,
    pub image_url: Option<String>,
    pub booking_url: Option<String>,
    /// Decision facts shown next to the price (all optional).
    pub room_size_sqm: Option<i64>,
    pub price_source: Option<String>,
    /// When the rate was read. A published price with no date is a lie waiting to happen.
    pub price_checked_at: Option<String>,
    pub free_cancel_until: Option<String>,
    pub rooms_left: Option<i64>,
    /// Bathtub in THIS room type: NULL = unverified, 1 = yes, 0 = no. Property-level
    /// filters lie (a 浴缸 filter can match via other room types) — this is per-room.
    pub has_bathtub: Option<i64>,
    /// Recommended order: 1 = first choice, 2.. = follow-up order. NULL = not ranked
    /// (not in the recommendation — e.g. fails a hard requirement).
    pub ranking: Option<i64>,
    /// 優劣比較與推薦理由 (ZH free text). Rendered on the candidate card and in the
    /// 推薦排序 block. NULL/empty = comparison content not written yet.
    pub notes: Option<String>,
    /// WGS84 coordinates for the dashboard's per-candidate location minimap.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Tax basis of the published rate (`PriceBasis`): NULL = 未確認.
    pub price_basis: Option<String>,
    pub updated_at: String,
}

/// Builder for `domestic_accommodations` WHERE clause with bound params.
/// Pattern mirrors `OfferFilter`: each predicate appends `col = ?N` + Value.
#[derive(Debug, Default)]
pub struct DomesticAccommodationFilter {
    conds: Vec<String>,
    params: Vec<libsql::Value>,
}

impl DomesticAccommodationFilter {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_placeholder(&self) -> usize {
        self.params.len() + 1
    }

    /// `destination = ?`
    pub fn destination(mut self, value: &str) -> Self {
        let n = self.next_placeholder();
        self.conds.push(format!("destination = ?{n}"));
        self.params.push(libsql::Value::Text(value.to_string()));
        self
    }

    /// `sea_view = 1` (no value bind, fixed predicate)
    pub fn sea_view_only(mut self) -> Self {
        self.conds.push("sea_view = 1".to_string());
        self
    }

    /// `hotel_name LIKE %value%` (bound, escaped by binding)
    pub fn hotel_like(mut self, value: &str) -> Self {
        let n = self.next_placeholder();
        self.conds.push(format!("hotel_name LIKE ?{n}"));
        self.params
            .push(libsql::Value::Text(format!("%{value}%")));
        self
    }

    pub fn build(self) -> DomesticAccommodationWhere {
        let clause = if self.conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.conds.join(" AND "))
        };
        DomesticAccommodationWhere {
            clause,
            params: self.params,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct DomesticAccommodationWhere {
    pub clause: String,
    pub params: Vec<libsql::Value>,
}

/// Query `domestic_accommodations` with parameterized WHERE + limit.
pub async fn query(
    conn: &Connection,
    filter: DomesticAccommodationFilter,
    limit: i64,
) -> Result<Vec<DomesticAccommodationRow>, String> {
    let built = filter.build();
    // `price_basis` is appended at the TAIL so existing column indices 0..=22
    // (and every `row.get(n)` consumer) keep their meaning.
    let sql = format!(
        "SELECT id, destination, hotel_name, room_type, sea_view, max_occupancy, price_twd, currency, breakfast_included, source, image_url, booking_url, room_size_sqm, price_source, price_checked_at, free_cancel_until, rooms_left, has_bathtub, ranking, notes, latitude, longitude, updated_at, price_basis \
         FROM domestic_accommodations {} ORDER BY price_twd ASC, hotel_name ASC LIMIT {limit}",
        built.clause
    );
    let mut rows = conn
        .query(&sql, built.params)
        .await
        .map_err(|e| format!("failed to query domestic_accommodations: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read domestic_accommodations row: {e}"))?
    {
        out.push(DomesticAccommodationRow {
            id: row.get(0).unwrap_or_default(),
            destination: row.get(1).unwrap_or_default(),
            hotel_name: row.get(2).unwrap_or_default(),
            room_type: row.get(3).unwrap_or_default(),
            sea_view: row.get(4).unwrap_or(0),
            max_occupancy: row.get(5).ok(),
            price_twd: row.get(6).unwrap_or(0),
            currency: row.get(7).unwrap_or_else(|_| "TWD".to_string()),
            breakfast_included: row.get(8).unwrap_or(0),
            source: row.get(9).ok(),
            image_url: row.get(10).ok(),
            booking_url: row.get(11).ok(),
            room_size_sqm: row.get(12).ok(),
            price_source: row.get(13).ok(),
            price_checked_at: row.get(14).ok(),
            free_cancel_until: row.get(15).ok(),
            rooms_left: row.get(16).ok(),
            has_bathtub: row.get(17).ok(),
            ranking: row.get(18).ok(),
            notes: row.get(19).ok(),
            latitude: row.get(20).ok(),
            longitude: row.get(21).ok(),
            updated_at: row.get(22).unwrap_or_default(),
            price_basis: row.get(23).ok(),
        });
    }
    Ok(out)
}

/// Row to INSERT via `add-accommodation` (slug-keyed reference data write).
#[derive(Debug, Clone)]
pub struct NewDomesticAccommodation {
    pub id: String,
    pub destination: String,
    pub hotel_name: String,
    pub room_type: String,
    pub sea_view: i64,
    pub max_occupancy: Option<i64>,
    pub price_twd: i64,
    pub breakfast_included: i64,
    pub source: Option<String>,
    pub image_url: Option<String>,
    pub booking_url: Option<String>,
    /// Optional decision facts, same columns update-accommodation writes. A rate
    /// quoted at add time must carry its read date — the CLI stamps
    /// price_checked_at = today when --price-source is given without --price-checked.
    pub room_size_sqm: Option<i64>,
    pub price_source: Option<String>,
    pub price_checked_at: Option<String>,
    pub free_cancel_until: Option<String>,
    pub rooms_left: Option<i64>,
    /// Per-room bathtub: None = unverified (NULL), Some(1)/Some(0) = verified yes/no.
    /// Valued flag `--bathtub yes|no` — an unverified room stays NULL, never a guessed 0.
    pub has_bathtub: Option<i64>,
    /// Recommended order at add time (optional — usually set later via update-accommodation
    /// once the comparison is written). 1 = first choice.
    pub ranking: Option<i64>,
    /// 優劣比較與推薦理由 at add time (optional).
    pub notes: Option<String>,
    /// WGS84 coordinates (optional) — geocode via Google Maps place search or
    /// OSM/Nominatim; the dashboard renders the location minimap from them.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Tax basis of the quoted rate (optional) — `incl`/`excl` as validated by
    /// [`PriceBasis::parse`]; None leaves the column NULL (未確認).
    pub price_basis: Option<String>,
}

/// INSERT OR IGNORE one row. Returns affected rows: 1 = inserted, 0 = id already
/// exists (natural dedup — NOT an error; the CLI surfaces it as "already exists").
pub async fn insert(conn: &Connection, row: &NewDomesticAccommodation) -> Result<u64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, max_occupancy, price_twd, currency, breakfast_included, source, image_url, booking_url, room_size_sqm, price_source, price_checked_at, free_cancel_until, rooms_left, has_bathtub, ranking, notes, latitude, longitude, price_basis, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'TWD', ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, datetime('now'))",
        libsql::params![
            row.id.clone(),
            row.destination.clone(),
            row.hotel_name.clone(),
            row.room_type.clone(),
            row.sea_view,
            row.max_occupancy,
            row.price_twd,
            row.breakfast_included,
            row.source.clone(),
            row.image_url.clone(),
            row.booking_url.clone(),
            row.room_size_sqm,
            row.price_source.clone(),
            row.price_checked_at.clone(),
            row.free_cancel_until.clone(),
            row.rooms_left,
            row.has_bathtub,
            row.ranking,
            row.notes.clone(),
            row.latitude,
            row.longitude,
            row.price_basis.clone(),
        ],
    )
    .await
    .map_err(|e| format!("domestic_accommodations INSERT failed: {e}"))
}

/// Build the UPDATE for the caller-supplied columns (pure — unit-testable).
/// `sets` is an ordered list of `(column, value)`; the caller guarantees the column
/// names are literals it owns (never user input), so only the VALUES are bound.
/// `updated_at` always bumps. Caller guarantees `sets` is non-empty.
pub fn build_update(id: &str, sets: &[(&str, libsql::Value)]) -> (String, Vec<libsql::Value>) {
    let mut frags: Vec<String> = Vec::new();
    let mut params: Vec<libsql::Value> = Vec::new();
    for (col, val) in sets {
        params.push(val.clone());
        frags.push(format!("{col} = ?{}", params.len()));
    }
    frags.push("updated_at = datetime('now')".to_string());
    params.push(libsql::Value::Text(id.to_string()));
    let sql = format!(
        "UPDATE domestic_accommodations SET {} WHERE id = ?{}",
        frags.join(", "),
        params.len()
    );
    (sql, params)
}

/// Like [`build_update`], plus the atomic basis guard for a `--price-source`
/// change: `price_basis` becomes NULL in the SAME statement when the stored
/// source differs from `new_source` (SQLite evaluates every right-hand side
/// against the OLD row), so a concurrent writer can never leave a basis that
/// belongs to the previous source. Caller guarantees `sets` has no
/// `price_basis` entry (an explicit basis wins and needs no guard).
pub fn build_update_guarding_basis(
    id: &str,
    sets: &[(&str, libsql::Value)],
    new_source: &str,
) -> (String, Vec<libsql::Value>) {
    let mut frags: Vec<String> = Vec::new();
    let mut params: Vec<libsql::Value> = Vec::new();
    for (col, val) in sets {
        params.push(val.clone());
        frags.push(format!("{col} = ?{}", params.len()));
    }
    params.push(libsql::Value::Text(new_source.to_string()));
    frags.push(format!(
        "price_basis = CASE WHEN price_source IS NOT ?{} THEN NULL ELSE price_basis END",
        params.len()
    ));
    frags.push("updated_at = datetime('now')".to_string());
    params.push(libsql::Value::Text(id.to_string()));
    let sql = format!(
        "UPDATE domestic_accommodations SET {} WHERE id = ?{}",
        frags.join(", "),
        params.len()
    );
    (sql, params)
}

/// UPDATE with the atomic source-change basis guard. Returns affected rows.
pub async fn update_fields_guarding_basis(
    conn: &Connection,
    id: &str,
    sets: &[(&str, libsql::Value)],
    new_source: &str,
) -> Result<u64, String> {
    let (sql, params) = build_update_guarding_basis(id, sets, new_source);
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("domestic_accommodations UPDATE failed: {e}"))
}

/// The stored `(price_source, price_basis)` of one row; `None` = unknown id.
/// Used only to word the "basis cleared" note — the clear itself is atomic in
/// [`update_fields_guarding_basis`].
pub async fn price_source_and_basis(
    conn: &Connection,
    id: &str,
) -> Result<Option<(Option<String>, Option<String>)>, String> {
    let mut rows = conn
        .query(
            "SELECT price_source, price_basis FROM domestic_accommodations WHERE id = ?1",
            libsql::params![id.to_string()],
        )
        .await
        .map_err(|e| format!("failed to read domestic_accommodations: {e}"))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read domestic_accommodations row: {e}"))?
    else {
        return Ok(None);
    };
    let src = row.get::<Option<String>>(0).map_err(|e| format!("price_source: {e}"))?;
    let basis = row.get::<Option<String>>(1).map_err(|e| format!("price_basis: {e}"))?;
    Ok(Some((src, basis)))
}

/// UPDATE the given columns by id. Returns affected rows (0 = unknown id).
pub async fn update_fields(
    conn: &Connection,
    id: &str,
    sets: &[(&str, libsql::Value)],
) -> Result<u64, String> {
    let (sql, params) = build_update(id, sets);
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("domestic_accommodations UPDATE failed: {e}"))
}

/// DELETE one row by id. Returns affected rows (0 = unknown id).
pub async fn delete_by_id(conn: &Connection, id: &str) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM domestic_accommodations WHERE id = ?1",
        libsql::params![id.to_string()],
    )
    .await
    .map_err(|e| format!("domestic_accommodations DELETE failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_empty_is_no_where() {
        let w = DomesticAccommodationFilter::new().build();
        assert_eq!(w.clause, "");
        assert!(w.params.is_empty());
    }

    #[test]
    fn filter_destination_and_sea_view() {
        let w = DomesticAccommodationFilter::new()
            .destination("jiufen")
            .sea_view_only()
            .build();
        assert_eq!(w.clause, "WHERE destination = ?1 AND sea_view = 1");
        assert_eq!(w.params.len(), 1);
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "jiufen"));
    }

    #[test]
    fn filter_hotel_like_is_bound() {
        let w = DomesticAccommodationFilter::new()
            .hotel_like("海論")
            .build();
        assert_eq!(w.clause, "WHERE hotel_name LIKE ?1");
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "%海論%"));
    }

    #[test]
    fn hotel_like_with_quote_is_bound_not_interpolated() {
        let w = DomesticAccommodationFilter::new()
            .hotel_like("a'b")
            .build();
        assert_eq!(w.clause, "WHERE hotel_name LIKE ?1");
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "%a'b%"));
    }

    fn txt(v: &str) -> libsql::Value {
        libsql::Value::Text(v.to_string())
    }

    #[test]
    fn build_update_two_columns() {
        let (sql, params) = build_update(
            "id1",
            &[("image_url", txt("https://img")), ("booking_url", txt("https://book"))],
        );
        assert_eq!(
            sql,
            "UPDATE domestic_accommodations SET image_url = ?1, booking_url = ?2, updated_at = datetime('now') WHERE id = ?3"
        );
        assert_eq!(params.len(), 3);
        assert!(matches!(&params[2], libsql::Value::Text(s) if s == "id1"));
    }

    #[test]
    fn build_update_single_column() {
        let (sql, params) = build_update("id2", &[("booking_url", txt("https://book"))]);
        assert_eq!(
            sql,
            "UPDATE domestic_accommodations SET booking_url = ?1, updated_at = datetime('now') WHERE id = ?2"
        );
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn build_update_binds_non_text_values_in_order() {
        let (sql, params) = build_update(
            "id3",
            &[
                ("room_size_sqm", libsql::Value::Integer(18)),
                ("rooms_left", libsql::Value::Integer(1)),
                ("price_source", txt("Booking.com")),
            ],
        );
        assert!(sql.starts_with(
            "UPDATE domestic_accommodations SET room_size_sqm = ?1, rooms_left = ?2, price_source = ?3, updated_at = datetime('now')"
        ));
        assert!(matches!(params[0], libsql::Value::Integer(18)));
        assert!(matches!(&params[2], libsql::Value::Text(s) if s == "Booking.com"));
        assert!(matches!(&params[3], libsql::Value::Text(s) if s == "id3"));
    }

    #[test]
    fn price_basis_parses_legal_values_and_rejects_junk() {
        assert_eq!(PriceBasis::parse("incl").unwrap(), PriceBasis::Incl);
        assert_eq!(PriceBasis::parse("excl").unwrap(), PriceBasis::Excl);
        // Junk (dirty db-exec value, typo'd flag) fails loud naming the legal set.
        for junk in ["", "incl ", "tax-free", "INCL", "含稅"] {
            assert!(
                PriceBasis::parse(junk)
                    .unwrap_err()
                    .contains("legal values: incl | excl"),
                "junk {junk:?} must be rejected"
            );
        }
        assert_eq!(PriceBasis::Incl.as_str(), "incl");
        assert_eq!(PriceBasis::Excl.as_str(), "excl");
    }

    #[test]
    fn guarded_update_clears_basis_in_the_same_statement() {
        let (sql, params) = build_update_guarding_basis(
            "idg",
            &[("price_source", txt("Agoda"))],
            "Agoda",
        );
        assert!(sql.contains(
            "price_basis = CASE WHEN price_source IS NOT ?2 THEN NULL ELSE price_basis END"
        ), "{sql}");
        assert!(sql.ends_with("WHERE id = ?3"), "{sql}");
        assert!(matches!(&params[1], libsql::Value::Text(s) if s == "Agoda"));
        assert!(matches!(&params[2], libsql::Value::Text(s) if s == "idg"));
    }

    #[test]
    fn build_update_carries_price_basis_text_and_null() {
        // The generic UPDATE path carries the new column both ways: a validated
        // value and the SQL NULL "clear" (update-accommodation --price-basis clear).
        let (sql, params) = build_update("idb", &[("price_basis", txt("incl"))]);
        assert!(sql.contains("price_basis = ?1"));
        assert!(matches!(&params[0], libsql::Value::Text(s) if s == "incl"));
        let (sql, params) = build_update("idb", &[("price_basis", libsql::Value::Null)]);
        assert!(sql.contains("price_basis = ?1"));
        assert!(matches!(params[0], libsql::Value::Null));
    }
}
