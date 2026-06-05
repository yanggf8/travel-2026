use libsql::{Connection, Value, params};
use std::env;
use std::path::PathBuf;

pub struct TravelDb {
    conn: Connection,
}

impl TravelDb {
    pub async fn connect() -> Result<Self, String> {
        let creds = TursoCreds::load()?;
        let db = libsql::Builder::new_remote(creds.url, creds.token)
            .build()
            .await
            .map_err(|err| format!("failed to connect to Turso: {err}"))?;
        let conn = db
            .connect()
            .map_err(|err| format!("failed to open Turso connection: {err}"))?;
        Ok(Self { conn })
    }


    pub async fn exec(&self, sql: &str) -> Result<u64, String> {
        self.conn
            .execute(sql, ())
            .await
            .map_err(|err| format!("Turso exec failed: {err}"))
    }

    /// Query returning column names + rows of plain string cells, for table
    /// rendering in the CLI (no JSON output).
    pub async fn query_table(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
        let mut rows = self
            .conn
            .query(sql, ())
            .await
            .map_err(|err| format!("Turso query failed: {err}"))?;
        let mut cols: Vec<String> = Vec::new();
        let mut out: Vec<Vec<String>> = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| format!("failed to read Turso row: {err}"))?
        {
            if cols.is_empty() {
                for idx in 0..row.column_count() {
                    cols.push(
                        row.column_name(idx)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("col_{idx}")),
                    );
                }
            }
            let mut cells = Vec::with_capacity(cols.len());
            for idx in 0..row.column_count() {
                let value = row
                    .get_value(idx)
                    .map_err(|err| format!("failed to read column {idx}: {err}"))?;
                cells.push(libsql_value_to_plain(value));
            }
            out.push(cells);
        }
        Ok((cols, out))
    }

    /// Ensure the captures table exists (plain-text raw_text, no JSON files).
    pub async fn ensure_captures_table(&self) -> Result<(), String> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS captures (
                    capture_id TEXT PRIMARY KEY,
                    source_id TEXT NOT NULL,
                    url TEXT,
                    title TEXT,
                    captured_at TEXT NOT NULL,
                    raw_text TEXT NOT NULL
                 )",
                (),
            )
            .await
            .map_err(|err| format!("failed to create captures table: {err}"))?;
        Ok(())
    }

    /// Store a capture as a row (raw_text is plain text — never a JSON file).
    pub async fn insert_capture(
        &self,
        capture_id: &str,
        source_id: &str,
        url: &str,
        title: &str,
        captured_at: &str,
        raw_text: &str,
    ) -> Result<(), String> {
        self.ensure_captures_table().await?;
        self.conn
            .execute(
                "INSERT INTO captures (capture_id, source_id, url, title, captured_at, raw_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(capture_id) DO UPDATE SET
                    source_id=excluded.source_id, url=excluded.url, title=excluded.title,
                    captured_at=excluded.captured_at, raw_text=excluded.raw_text",
                params![
                    capture_id.to_string(),
                    source_id.to_string(),
                    url.to_string(),
                    title.to_string(),
                    captured_at.to_string(),
                    raw_text.to_string(),
                ],
            )
            .await
            .map_err(|err| format!("failed to insert capture: {err}"))?;
        Ok(())
    }

    /// Read a stored capture's source_id + raw_text by capture_id (for parsing).
    pub async fn get_capture(&self, capture_id: &str) -> Result<Option<(String, String, String)>, String> {
        let mut rows = self
            .conn
            .query(
                "SELECT source_id, url, raw_text FROM captures WHERE capture_id = ?1",
                params![capture_id.to_string()],
            )
            .await
            .map_err(|err| format!("failed to query capture: {err}"))?;
        if let Some(row) = rows
            .next()
            .await
            .map_err(|err| format!("failed to read capture row: {err}"))?
        {
            let source_id: String = row.get(0).map_err(|e| format!("col source_id: {e}"))?;
            let url: String = row.get(1).unwrap_or_default();
            let raw_text: String = row.get(2).map_err(|e| format!("col raw_text: {e}"))?;
            Ok(Some((source_id, url, raw_text)))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_offer(&self, row: &OfferRow) -> Result<u64, String> {
        self.conn
            .execute(
                "INSERT INTO offers (
                    id, source_file, source_id, type, name, price_per_person, currency,
                    region, destination, departure_date, return_date, nights, availability,
                    hotel_name, airline, scraped_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(id, scraped_at) DO NOTHING",
                params![
                    row.id.clone(),
                    row.source_file.clone(),
                    row.source_id.clone(),
                    row.kind.clone(),
                    row.name.clone(),
                    row.price_per_person,
                    row.currency.clone(),
                    row.region.clone(),
                    row.destination.clone(),
                    row.departure_date.clone(),
                    row.return_date.clone(),
                    row.nights,
                    row.availability.clone(),
                    row.hotel_name.clone(),
                    row.airline.clone(),
                    row.scraped_at.clone(),
                ],
            )
            .await
            .map_err(|err| format!("failed to insert offer {}: {err}", row.id))
    }

    pub async fn ensure_parser_rules_table(&self) -> Result<u64, String> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS parser_rules (
                    source_id TEXT PRIMARY KEY,
                    product_kind TEXT DEFAULT 'fit',
                    date_range_rx TEXT NOT NULL,
                    nights_rx TEXT NOT NULL,
                    nights_is_days INTEGER DEFAULT 0,
                    price_marker TEXT NOT NULL,
                    price_amount_rx TEXT NOT NULL,
                    price_basis TEXT DEFAULT 'total',
                    pax_divisor INTEGER DEFAULT 2,
                    flight_rx TEXT NOT NULL,
                    hotel_anchor_rx TEXT NOT NULL,
                    currency TEXT DEFAULT 'TWD',
                    has_custom_parser INTEGER DEFAULT 0,
                    source_url TEXT,
                    fetched_at TEXT
                 )",
                (),
            )
            .await
            .map_err(|err| format!("failed to create parser_rules table: {err}"))
    }

    pub async fn upsert_parser_rule(&self, rule: &ParserRule) -> Result<u64, String> {
        self.conn
            .execute(
                "INSERT INTO parser_rules (
                    source_id, product_kind, date_range_rx, nights_rx, nights_is_days,
                    price_marker, price_amount_rx, price_basis, pax_divisor, flight_rx,
                    hotel_anchor_rx, currency, has_custom_parser, source_url, fetched_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(source_id) DO UPDATE SET
                    product_kind=excluded.product_kind,
                    date_range_rx=excluded.date_range_rx,
                    nights_rx=excluded.nights_rx,
                    nights_is_days=excluded.nights_is_days,
                    price_marker=excluded.price_marker,
                    price_amount_rx=excluded.price_amount_rx,
                    price_basis=excluded.price_basis,
                    pax_divisor=excluded.pax_divisor,
                    flight_rx=excluded.flight_rx,
                    hotel_anchor_rx=excluded.hotel_anchor_rx,
                    currency=excluded.currency,
                    has_custom_parser=excluded.has_custom_parser,
                    source_url=excluded.source_url,
                    fetched_at=excluded.fetched_at",
                params![
                    rule.source_id.clone(),
                    rule.product_kind.clone(),
                    rule.date_range_rx.clone(),
                    rule.nights_rx.clone(),
                    if rule.nights_is_days { 1_i64 } else { 0_i64 },
                    rule.price_marker.clone(),
                    rule.price_amount_rx.clone(),
                    rule.price_basis.clone(),
                    rule.pax_divisor,
                    rule.flight_rx.clone(),
                    rule.hotel_anchor_rx.clone(),
                    rule.currency.clone(),
                    if rule.has_custom_parser { 1_i64 } else { 0_i64 },
                    rule.source_url.clone(),
                    rule.fetched_at.clone(),
                ],
            )
            .await
            .map_err(|err| {
                format!(
                    "failed to upsert parser_rules row for {}: {err}",
                    rule.source_id
                )
            })
    }

    pub async fn parser_rule(&self, source_id: &str) -> Result<Option<ParserRule>, String> {
        let mut rows = self
            .conn
            .query(
                "SELECT
                    source_id,
                    COALESCE(product_kind, 'fit'),
                    date_range_rx,
                    nights_rx,
                    COALESCE(nights_is_days, 0),
                    price_marker,
                    price_amount_rx,
                    COALESCE(price_basis, 'total'),
                    COALESCE(pax_divisor, 2),
                    flight_rx,
                    hotel_anchor_rx,
                    COALESCE(currency, 'TWD'),
                    COALESCE(has_custom_parser, 0),
                    COALESCE(source_url, ''),
                    COALESCE(fetched_at, '')
                 FROM parser_rules
                 WHERE source_id = ?1",
                params![source_id.to_string()],
            )
            .await
            .map_err(|err| format!("failed to query parser_rules for {source_id}: {err}"))?;
        let Some(row) = rows
            .next()
            .await
            .map_err(|err| format!("failed to read parser_rules row for {source_id}: {err}"))?
        else {
            return Ok(None);
        };

        let source_url: String = row
            .get(13)
            .map_err(|err| format!("failed to read parser_rules.source_url: {err}"))?;
        let fetched_at: String = row
            .get(14)
            .map_err(|err| format!("failed to read parser_rules.fetched_at: {err}"))?;
        Ok(Some(ParserRule {
            source_id: row
                .get(0)
                .map_err(|err| format!("failed to read parser_rules.source_id: {err}"))?,
            product_kind: row
                .get(1)
                .map_err(|err| format!("failed to read parser_rules.product_kind: {err}"))?,
            date_range_rx: row
                .get(2)
                .map_err(|err| format!("failed to read parser_rules.date_range_rx: {err}"))?,
            nights_rx: row
                .get(3)
                .map_err(|err| format!("failed to read parser_rules.nights_rx: {err}"))?,
            nights_is_days: row
                .get::<i64>(4)
                .map_err(|err| format!("failed to read parser_rules.nights_is_days: {err}"))?
                != 0,
            price_marker: row
                .get(5)
                .map_err(|err| format!("failed to read parser_rules.price_marker: {err}"))?,
            price_amount_rx: row
                .get(6)
                .map_err(|err| format!("failed to read parser_rules.price_amount_rx: {err}"))?,
            price_basis: row
                .get(7)
                .map_err(|err| format!("failed to read parser_rules.price_basis: {err}"))?,
            pax_divisor: row
                .get(8)
                .map_err(|err| format!("failed to read parser_rules.pax_divisor: {err}"))?,
            flight_rx: row
                .get(9)
                .map_err(|err| format!("failed to read parser_rules.flight_rx: {err}"))?,
            hotel_anchor_rx: row
                .get(10)
                .map_err(|err| format!("failed to read parser_rules.hotel_anchor_rx: {err}"))?,
            currency: row
                .get(11)
                .map_err(|err| format!("failed to read parser_rules.currency: {err}"))?,
            has_custom_parser: row
                .get::<i64>(12)
                .map_err(|err| format!("failed to read parser_rules.has_custom_parser: {err}"))?
                != 0,
            source_url: none_if_empty(source_url),
            fetched_at: none_if_empty(fetched_at),
        }))
    }
}

#[derive(Debug, Clone)]
pub struct ParserRule {
    pub source_id: String,
    pub product_kind: String,
    pub date_range_rx: String,
    pub nights_rx: String,
    pub nights_is_days: bool,
    pub price_marker: String,
    pub price_amount_rx: String,
    pub price_basis: String,
    pub pax_divisor: i64,
    pub flight_rx: String,
    pub hotel_anchor_rx: String,
    pub currency: String,
    pub has_custom_parser: bool,
    pub source_url: Option<String>,
    pub fetched_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OfferRow {
    pub id: String,
    pub source_file: Option<String>,
    pub source_id: String,
    pub kind: String,
    pub name: Option<String>,
    pub price_per_person: Option<i64>,
    pub currency: Option<String>,
    pub region: Option<String>,
    pub destination: Option<String>,
    pub departure_date: Option<String>,
    pub return_date: Option<String>,
    pub nights: Option<i64>,
    pub availability: Option<String>,
    pub hotel_name: Option<String>,
    pub airline: Option<String>,
    pub scraped_at: Option<String>,
}

struct TursoCreds {
    url: String,
    token: String,
}

impl TursoCreds {
    fn load() -> Result<Self, String> {
        let mut url = env::var("TURSO_URL").ok().filter(|v| !v.trim().is_empty());
        let mut token = env::var("TURSO_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty());

        if url.is_none() || token.is_none() {
            if let Some(env_body) = read_repo_env() {
                for line in env_body.lines() {
                    if url.is_none() {
                        if let Some(value) = line.strip_prefix("TURSO_URL=") {
                            url = Some(unquote_env_value(value));
                        }
                    }
                    if token.is_none() {
                        if let Some(value) = line.strip_prefix("TURSO_TOKEN=") {
                            token = Some(unquote_env_value(value));
                        }
                    }
                }
            }
        }

        let url = url.ok_or_else(|| {
            "missing TURSO_URL; set it in the environment or repo .env".to_string()
        })?;
        let token = token.ok_or_else(|| {
            "missing TURSO_TOKEN; set it in the environment or repo .env".to_string()
        })?;
        Ok(Self { url, token })
    }
}

fn read_repo_env() -> Option<String> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = path.join(".env");
        if let Ok(body) = std::fs::read_to_string(candidate) {
            return Some(body);
        }
        if !path.pop() {
            break;
        }
    }
    None
}

fn unquote_env_value(value: &str) -> String {
    let trimmed = value.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len().saturating_sub(1)].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Render a libsql cell as a plain string for table output (no JSON).
fn libsql_value_to_plain(value: Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(value) => value.to_string(),
        Value::Real(value) => value.to_string(),
        Value::Text(value) => value,
        Value::Blob(value) => format!("<blob:{} bytes>", value.len()),
    }
}

fn none_if_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}
