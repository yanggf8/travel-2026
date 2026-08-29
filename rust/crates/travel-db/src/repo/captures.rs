use crate::checksum::sha256_hex;
use libsql::Connection;

#[derive(Debug, Clone)]
pub struct CaptureRow {
    pub capture_id: String,
    pub source_id: String,
    pub url: Option<String>,
    pub captured_at: String,
    pub raw_text: String,
}

/// Load a capture by id. Returns `None` when not found.
pub async fn get(conn: &Connection, capture_id: &str) -> Result<Option<CaptureRow>, String> {
    let mut rows = conn
        .query(
            "SELECT capture_id, source_id, url, captured_at, raw_text FROM captures WHERE capture_id = ?1",
            libsql::params![capture_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(CaptureRow {
        capture_id: row.get(0).map_err(|e| e.to_string())?,
        source_id: row.get(1).map_err(|e| e.to_string())?,
        url: row.get(2).ok(),
        captured_at: row.get(3).map_err(|e| e.to_string())?,
        raw_text: row.get(4).map_err(|e| e.to_string())?,
    }))
}

/// SHA-256 hex of capture raw text at parse time.
pub fn capture_checksum(raw_text: &str) -> String {
    sha256_hex(raw_text)
}