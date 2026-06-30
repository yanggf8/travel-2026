use libsql::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductTypeInputRow {
    pub product_type: String,
    pub input_name: String,
    pub input_class: String,
    pub required: i64,
    pub default_source: Option<String>,
    pub sort_order: i64,
}

/// Load the canonical input contract for a product_type, ordered by sort_order then input_name.
pub async fn list_for_type(
    conn: &Connection,
    product_type: &str,
) -> Result<Vec<ProductTypeInputRow>, String> {
    let mut rows = conn
        .query(
            "SELECT product_type, input_name, input_class, required, default_source, sort_order \
             FROM product_type_inputs \
             WHERE product_type = ?1 \
             ORDER BY sort_order, input_name",
            libsql::params![product_type.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(ProductTypeInputRow {
            product_type: row.get(0).map_err(|e| e.to_string())?,
            input_name: row.get(1).map_err(|e| e.to_string())?,
            input_class: row.get(2).map_err(|e| e.to_string())?,
            required: row.get(3).unwrap_or(1),
            default_source: row.get(4).ok(),
            sort_order: row.get(5).unwrap_or(0),
        });
    }
    Ok(out)
}