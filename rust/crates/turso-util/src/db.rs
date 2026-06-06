use libsql::{Builder, Database};

use crate::error::{Error, Result};

pub async fn database(url: &str, token: Option<&str>) -> Result<Database> {
    if url.starts_with("file:") {
        let path = url.trim_start_matches("file:");
        Builder::new_local(path)
            .build()
            .await
            .map_err(|e| Error::turso(e.to_string()))
    } else {
        let token = token.ok_or_else(|| Error::auth("a Turso auth token is required"))?;
        Builder::new_remote(url.to_string(), token.to_string())
            .build()
            .await
            .map_err(|e| Error::turso(e.to_string()))
    }
}
