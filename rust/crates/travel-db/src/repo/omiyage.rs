//! Omiyage (souvenir) recommendation + purchase-location reference data.
//!
//! Slug-keyed GLOBAL destination-ref tables — NO audit triad / NO plans.version.
//! Domain writes only; callers stamp `fetched_at`.
//!
//! Write path is **one atomic transactional writer** (`write_item_and_location`):
//! BEGIN → validate slug + same-slug POI → read item →
//!   absent: require full item bundle → plain INSERT item → INSERT OR REPLACE location
//!   present: match only SUPPLIED item flags → location upsert only
//! → COMMIT on expected affected counts; ROLLBACK otherwise.

use libsql::Connection;

use super::destination_ref;

// ── types ──────────────────────────────────────────────────────────────────

/// Stored item fields used for match comparison (`fetched_at` excluded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmiyageItem {
    pub name: String,
    pub category: String,
    pub notes: Option<String>,
    pub source_url: String,
    pub confidence: String,
}

/// Optional item-bundle flags supplied by the CLI on a write.
/// `None` = not supplied (do not compare / do not require on existing item).
#[derive(Debug, Clone, Default)]
pub struct ItemFlags<'a> {
    pub name: Option<&'a str>,
    pub category: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub source_url: Option<&'a str>,
    pub confidence: Option<&'a str>,
}

/// Location half of a write (always required).
#[derive(Debug, Clone)]
pub struct LocationInput<'a> {
    pub poi_id: &'a str,
    pub purchase_note: Option<&'a str>,
    pub source_url: &'a str,
    pub confidence: &'a str,
}

/// Outcome of a successful atomic write (CLI uses this for plain-text messaging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    CreatedItemAndLocation,
    UpsertedLocationOnly,
}

/// One joined row from `query_omiyage`. LEFT JOIN pois → `poi_*` as Option so
/// the command layer can fail-loud on orphan locations (NULL title etc.).
#[derive(Debug, Clone)]
pub struct OmiyageRow {
    pub item_id: String,
    pub name: String,
    pub category: String,
    pub item_notes: Option<String>,
    pub item_source_url: String,
    pub item_confidence: String,
    pub item_fetched_at: String,
    pub poi_id: String,
    pub poi_title: Option<String>,
    pub area: Option<String>,
    pub station: Option<String>,
    pub address: Option<String>,
    pub hours: Option<String>,
    pub purchase_note: Option<String>,
    pub loc_source_url: String,
    pub loc_confidence: String,
    pub loc_fetched_at: String,
}

// ── public reads ───────────────────────────────────────────────────────────

/// `SELECT 1 FROM destination_config WHERE slug=?1`.
pub async fn config_slug_exists(conn: &Connection, slug: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM destination_config WHERE slug = ?1",
            libsql::params![slug.to_string()],
        )
        .await
        .map_err(|e| format!("destination_config existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("destination_config existence row read failed: {e}"))?
        .is_some())
}

/// Read one item by `(slug, item_id)` — match visibility / tests.
/// `fetched_at` is not returned (excluded from match comparison).
pub async fn read_item(
    conn: &Connection,
    slug: &str,
    item_id: &str,
) -> Result<Option<OmiyageItem>, String> {
    let mut rows = conn
        .query(
            "SELECT name, category, notes, source_url, confidence \
             FROM destination_omiyage_items \
             WHERE slug = ?1 AND item_id = ?2",
            libsql::params![slug.to_string(), item_id.to_string()],
        )
        .await
        .map_err(|e| format!("destination_omiyage_items read failed: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("destination_omiyage_items row read failed: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some(OmiyageItem {
        name: r.get(0).unwrap_or_default(),
        category: r.get(1).unwrap_or_default(),
        notes: opt_string(&r, 2),
        source_url: r.get(3).unwrap_or_default(),
        confidence: r.get(4).unwrap_or_default(),
    }))
}

/// Items ⨝ locations ⟕ destination_pois for one destination slug.
/// Ordered by category, name, item_id, poi title, poi_id.
pub async fn query_omiyage(conn: &Connection, slug: &str) -> Result<Vec<OmiyageRow>, String> {
    let mut rows = conn
        .query(
            "SELECT i.item_id, i.name, i.category, i.notes, i.source_url, i.confidence, i.fetched_at, \
                    l.poi_id, p.title, p.area, p.nearest_station, p.address, p.hours, \
                    l.purchase_note, l.source_url, l.confidence, l.fetched_at \
             FROM destination_omiyage_items i \
             JOIN destination_omiyage_locations l ON l.slug = i.slug AND l.item_id = i.item_id \
             LEFT JOIN destination_pois p ON p.slug = l.slug AND p.poi_id = l.poi_id \
             WHERE i.slug = ?1 \
             ORDER BY i.category, i.name, i.item_id, p.title, l.poi_id",
            libsql::params![slug.to_string()],
        )
        .await
        .map_err(|e| format!("query_omiyage failed: {e}"))?;

    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("query_omiyage row read failed: {e}"))?
    {
        out.push(OmiyageRow {
            item_id: r.get(0).unwrap_or_default(),
            name: r.get(1).unwrap_or_default(),
            category: r.get(2).unwrap_or_default(),
            item_notes: opt_string(&r, 3),
            item_source_url: r.get(4).unwrap_or_default(),
            item_confidence: r.get(5).unwrap_or_default(),
            item_fetched_at: r.get(6).unwrap_or_default(),
            poi_id: r.get(7).unwrap_or_default(),
            poi_title: opt_string(&r, 8),
            area: opt_string(&r, 9),
            station: opt_string(&r, 10),
            address: opt_string(&r, 11),
            hours: opt_string(&r, 12),
            purchase_note: opt_string(&r, 13),
            loc_source_url: r.get(14).unwrap_or_default(),
            loc_confidence: r.get(15).unwrap_or_default(),
            loc_fetched_at: r.get(16).unwrap_or_default(),
        });
    }
    Ok(out)
}

// ── atomic writer ──────────────────────────────────────────────────────────

/// Atomic write per plan Task 2 / spec L64–71.
///
/// `BEGIN` → `config_slug_exists` + `destination_ref::poi_coords_exists` →
/// `read_item` → insert-or-match + location upsert → `COMMIT` / `ROLLBACK`.
///
/// Location upsert uses `INSERT OR REPLACE`; libsql may report affected 1 or 2
/// (delete+insert). We assert `affected >= 1` (fail only on 0). Plain item
/// `INSERT` expects exactly 1.
pub async fn write_item_and_location(
    conn: &Connection,
    slug: &str,
    item_id: &str,
    item: ItemFlags<'_>,
    location: LocationInput<'_>,
    fetched_at: &str,
) -> Result<WriteOutcome, String> {
    conn.execute("BEGIN", libsql::params![])
        .await
        .map_err(|e| format!("omiyage BEGIN failed: {e}"))?;

    let result = write_item_and_location_inner(conn, slug, item_id, item, location, fetched_at).await;

    match result {
        Ok(outcome) => {
            conn.execute("COMMIT", libsql::params![])
                .await
                .map_err(|e| format!("omiyage COMMIT failed: {e}"))?;
            Ok(outcome)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", libsql::params![]).await;
            Err(e)
        }
    }
}

async fn write_item_and_location_inner(
    conn: &Connection,
    slug: &str,
    item_id: &str,
    item: ItemFlags<'_>,
    location: LocationInput<'_>,
    fetched_at: &str,
) -> Result<WriteOutcome, String> {
    if !config_slug_exists(conn, slug).await? {
        return Err(format!(
            "destination_config has no slug '{slug}' — register the destination first"
        ));
    }
    // REUSE destination_ref::poi_coords_exists — do NOT invent a second POI check.
    if !destination_ref::poi_coords_exists(conn, slug, location.poi_id).await? {
        return Err(format!(
            "destination_pois has no (slug='{slug}', poi_id='{}')",
            location.poi_id
        ));
    }

    let existing = read_item(conn, slug, item_id).await?;
    let outcome = match existing {
        None => {
            // New item: full bundle required (name/category/source_url/confidence present + non-blank).
            let name = require_nonblank_flag(item.name, "name")?;
            let category = require_nonblank_flag(item.category, "category")?;
            let source_url = require_nonblank_flag(item.source_url, "source_url")?;
            let confidence = require_nonblank_flag(item.confidence, "confidence")?;
            let notes = item.notes.filter(|s| !s.is_empty());

            let n = insert_item(
                conn,
                slug,
                item_id,
                name,
                category,
                notes,
                source_url,
                confidence,
                fetched_at,
            )
            .await?;
            if n != 1 {
                return Err(format!(
                    "destination_omiyage_items INSERT affected {n} rows (expected 1)"
                ));
            }

            let nloc = upsert_location(conn, slug, item_id, &location, fetched_at).await?;
            // INSERT OR REPLACE may report 1 (insert) or 2 (delete+insert); fail only on 0.
            if nloc < 1 {
                return Err(format!(
                    "destination_omiyage_locations upsert affected {nloc} rows (expected >= 1)"
                ));
            }
            WriteOutcome::CreatedItemAndLocation
        }
        Some(stored) => {
            // Existing item: do NOT write the item; only SUPPLIED flags must match.
            match_supplied_item_flags(&item, &stored)?;

            let nloc = upsert_location(conn, slug, item_id, &location, fetched_at).await?;
            if nloc < 1 {
                return Err(format!(
                    "destination_omiyage_locations upsert affected {nloc} rows (expected >= 1)"
                ));
            }
            WriteOutcome::UpsertedLocationOnly
        }
    };
    Ok(outcome)
}

// ── private helpers (transaction-internal only) ────────────────────────────

fn require_nonblank_flag<'a>(val: Option<&'a str>, field: &str) -> Result<&'a str, String> {
    match val {
        Some(s) if !s.trim().is_empty() => Ok(s),
        Some(_) => Err(format!(
            "new omiyage item requires non-blank {field} (item bundle incomplete)"
        )),
        None => Err(format!(
            "new omiyage item requires --{field} (item bundle incomplete)"
        )),
    }
}

/// Compare only SUPPLIED ItemFlags fields against the stored item (exact match).
/// Unsupplied (`None`) fields are not compared.
fn match_supplied_item_flags(item: &ItemFlags<'_>, stored: &OmiyageItem) -> Result<(), String> {
    if let Some(name) = item.name {
        if name != stored.name {
            return Err(format!(
                "omiyage item name mismatch: supplied '{name}' != stored '{}'",
                stored.name
            ));
        }
    }
    if let Some(category) = item.category {
        if category != stored.category {
            return Err(format!(
                "omiyage item category mismatch: supplied '{category}' != stored '{}'",
                stored.category
            ));
        }
    }
    if let Some(notes) = item.notes {
        let stored_notes = stored.notes.as_deref().unwrap_or("");
        if notes != stored_notes {
            return Err(format!(
                "omiyage item notes mismatch: supplied '{notes}' != stored '{stored_notes}'"
            ));
        }
    }
    if let Some(source_url) = item.source_url {
        if source_url != stored.source_url {
            return Err(format!(
                "omiyage item source_url mismatch: supplied '{source_url}' != stored '{}'",
                stored.source_url
            ));
        }
    }
    if let Some(confidence) = item.confidence {
        if confidence != stored.confidence {
            return Err(format!(
                "omiyage item confidence mismatch: supplied '{confidence}' != stored '{}'",
                stored.confidence
            ));
        }
    }
    Ok(())
}

/// Plain INSERT into `destination_omiyage_items` (NOT REPLACE).
#[allow(clippy::too_many_arguments)]
async fn insert_item(
    conn: &Connection,
    slug: &str,
    item_id: &str,
    name: &str,
    category: &str,
    notes: Option<&str>,
    source_url: &str,
    confidence: &str,
    fetched_at: &str,
) -> Result<u64, String> {
    conn.execute(
        "INSERT INTO destination_omiyage_items \
         (slug, item_id, name, category, notes, source_url, fetched_at, confidence) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        libsql::params![
            slug.to_string(),
            item_id.to_string(),
            name.to_string(),
            category.to_string(),
            notes,
            source_url.to_string(),
            fetched_at.to_string(),
            confidence.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("destination_omiyage_items INSERT failed: {e}"))
}

/// INSERT OR REPLACE into `destination_omiyage_locations` on `(slug, item_id, poi_id)`.
async fn upsert_location(
    conn: &Connection,
    slug: &str,
    item_id: &str,
    location: &LocationInput<'_>,
    fetched_at: &str,
) -> Result<u64, String> {
    conn.execute(
        "INSERT OR REPLACE INTO destination_omiyage_locations \
         (slug, item_id, poi_id, purchase_note, source_url, fetched_at, confidence) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        libsql::params![
            slug.to_string(),
            item_id.to_string(),
            location.poi_id.to_string(),
            location.purchase_note,
            location.source_url.to_string(),
            fetched_at.to_string(),
            location.confidence.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("destination_omiyage_locations INSERT OR REPLACE failed: {e}"))
}

fn opt_string(row: &libsql::Row, idx: i32) -> Option<String> {
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}
