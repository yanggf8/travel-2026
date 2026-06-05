use libsql::{Connection, Value, params};
use serde_json::{Map, Value as JsonValue};
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

    pub async fn query_json(&self, sql: &str) -> Result<JsonValue, String> {
        let mut rows = self
            .conn
            .query(sql, ())
            .await
            .map_err(|err| format!("Turso query failed: {err}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| format!("failed to read Turso row: {err}"))?
        {
            let mut object = Map::new();
            for idx in 0..row.column_count() {
                let name = row
                    .column_name(idx)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("col_{idx}"));
                let value = row
                    .get_value(idx)
                    .map_err(|err| format!("failed to read column {idx}: {err}"))?;
                object.insert(name, libsql_value_to_json(value));
            }
            out.push(JsonValue::Object(object));
        }
        Ok(JsonValue::Array(out))
    }

    pub async fn exec(&self, sql: &str) -> Result<u64, String> {
        self.conn
            .execute(sql, ())
            .await
            .map_err(|err| format!("Turso exec failed: {err}"))
    }

    pub async fn insert_offer(&self, row: &OfferRow) -> Result<u64, String> {
        self.conn
            .execute(
                "INSERT INTO offers (
                    id, source_file, source_id, type, name, price_per_person, currency,
                    region, destination, departure_date, return_date, nights, availability,
                    hotel_name, airline, raw_data, scraped_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT(id) DO UPDATE SET
                    source_file=excluded.source_file,
                    source_id=excluded.source_id,
                    type=excluded.type,
                    name=excluded.name,
                    price_per_person=excluded.price_per_person,
                    currency=excluded.currency,
                    region=excluded.region,
                    destination=excluded.destination,
                    departure_date=excluded.departure_date,
                    return_date=excluded.return_date,
                    nights=excluded.nights,
                    availability=excluded.availability,
                    hotel_name=excluded.hotel_name,
                    airline=excluded.airline,
                    raw_data=excluded.raw_data,
                    scraped_at=excluded.scraped_at",
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
                    row.raw_data.clone(),
                    row.scraped_at.clone(),
                ],
            )
            .await
            .map_err(|err| format!("failed to insert offer {}: {err}", row.id))
    }
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
    pub raw_data: String,
    pub scraped_at: Option<String>,
}

impl OfferRow {
    pub fn dry_run_sql(&self) -> String {
        let cols = [
            "id",
            "source_file",
            "source_id",
            "type",
            "name",
            "price_per_person",
            "currency",
            "region",
            "destination",
            "departure_date",
            "return_date",
            "nights",
            "availability",
            "hotel_name",
            "airline",
            "raw_data",
            "scraped_at",
        ];
        let values = [
            sql_text(Some(&self.id)),
            sql_text(self.source_file.as_deref()),
            sql_text(Some(&self.source_id)),
            sql_text(Some(&self.kind)),
            sql_text(self.name.as_deref()),
            sql_int(self.price_per_person),
            sql_text(self.currency.as_deref()),
            sql_text(self.region.as_deref()),
            sql_text(self.destination.as_deref()),
            sql_text(self.departure_date.as_deref()),
            sql_text(self.return_date.as_deref()),
            sql_int(self.nights),
            sql_text(self.availability.as_deref()),
            sql_text(self.hotel_name.as_deref()),
            sql_text(self.airline.as_deref()),
            sql_text(Some(&self.raw_data)),
            sql_text(self.scraped_at.as_deref()),
        ];
        format!(
            "INSERT INTO offers ({}) VALUES ({}) ON CONFLICT(id) DO UPDATE SET source_file=excluded.source_file, source_id=excluded.source_id, type=excluded.type, name=excluded.name, price_per_person=excluded.price_per_person, currency=excluded.currency, region=excluded.region, destination=excluded.destination, departure_date=excluded.departure_date, return_date=excluded.return_date, nights=excluded.nights, availability=excluded.availability, hotel_name=excluded.hotel_name, airline=excluded.airline, raw_data=excluded.raw_data, scraped_at=excluded.scraped_at;",
            cols.join(","),
            values.join(",")
        )
    }
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

fn libsql_value_to_json(value: Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Integer(value) => JsonValue::from(value),
        Value::Real(value) => JsonValue::from(value),
        Value::Text(value) => JsonValue::from(value),
        Value::Blob(value) => JsonValue::from(format!("<blob:{} bytes>", value.len())),
    }
}

fn sql_text(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("'{}'", value.replace('\'', "''")),
        None => "NULL".to_string(),
    }
}

fn sql_int(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}
