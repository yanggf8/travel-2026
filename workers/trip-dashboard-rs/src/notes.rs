use crate::{render, turso};
use note_core::{is_image_id, is_note_id, is_note_token, NOTE_PUBLIC_COLUMNS};
use std::collections::HashMap;
use worker::*;

#[derive(Debug, PartialEq, Eq)]
pub enum NoteRoute {
    OwnerList,
    OwnerNew,
    OwnerEdit(String),
    OwnerPost(String),
    SharePage(String),
    ShareImage(String, String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum NoteAccess {
    Allow,
    NotFound,
}

pub struct AccessRow {
    pub token_note_id: Option<String>,
    pub token_status: Option<String>,
    pub note_deleted: bool,
    pub image_exists: bool,
}

pub fn is_note_path(path: &str) -> bool {
    path == "/notes" || path.starts_with("/notes/") || path.starts_with("/n/")
}

pub fn parse_note_route(path: &str) -> Option<NoteRoute> {
    if path == "/notes" {
        return Some(NoteRoute::OwnerList);
    }
    if path == "/notes/new" {
        return Some(NoteRoute::OwnerNew);
    }
    if let Some(rest) = path.strip_prefix("/notes/") {
        let segments: Vec<_> = rest.split('/').collect();
        if segments.len() == 2 && is_note_id(segments[0]) && segments[1] == "edit" {
            return Some(NoteRoute::OwnerEdit(segments[0].into()));
        }
        return None;
    }
    let rest = path.strip_prefix("/n/")?;
    let segments: Vec<_> = rest.split('/').collect();
    match segments.as_slice() {
        [id] if is_note_id(id) => Some(NoteRoute::SharePage((*id).into())),
        [id, "img", image] if is_note_id(id) && is_image_id(image) => {
            Some(NoteRoute::ShareImage((*id).into(), (*image).into()))
        }
        _ => None,
    }
}

pub fn note_token(query: &HashMap<String, String>) -> Option<&str> {
    query.get("t").map(String::as_str)
}

pub fn decide_note_access(
    route: &NoteRoute,
    is_owner: bool,
    token: Option<&str>,
    row: Option<&AccessRow>,
) -> NoteAccess {
    if matches!(
        route,
        NoteRoute::OwnerList
            | NoteRoute::OwnerNew
            | NoteRoute::OwnerEdit(_)
            | NoteRoute::OwnerPost(_)
    ) {
        return if is_owner {
            NoteAccess::Allow
        } else {
            NoteAccess::NotFound
        };
    }
    let Some(row) = row else {
        return NoteAccess::NotFound;
    };
    if row.note_deleted || matches!(route, NoteRoute::ShareImage(_, _) if !row.image_exists) {
        return NoteAccess::NotFound;
    }
    if is_owner {
        return NoteAccess::Allow;
    }
    let id = match route {
        NoteRoute::SharePage(id) | NoteRoute::ShareImage(id, _) => id,
        _ => unreachable!(),
    };
    if token.is_some_and(is_note_token)
        && row.token_status.as_deref() == Some("active")
        && row.token_note_id.as_deref() == Some(id)
    {
        NoteAccess::Allow
    } else {
        NoteAccess::NotFound
    }
}

#[derive(Clone, Copy)]
pub enum NoteResponseKind<'a> {
    SharePage,
    Image(&'a str),
    NotFound,
}

/// Complete final headers: `with_headers` replaces the response's existing set.
pub fn note_header_values(kind: NoteResponseKind<'_>) -> Vec<(&'static str, &str)> {
    let mut headers = NOTE_HEADERS.to_vec();
    match kind {
        NoteResponseKind::SharePage => headers.push(("Content-Type", "text/html; charset=utf-8")),
        NoteResponseKind::Image(content_type) => {
            headers.push(("Content-Type", content_type));
            headers.push(("Content-Disposition", "inline"));
        }
        NoteResponseKind::NotFound => headers.push(("Content-Type", "text/plain; charset=utf-8")),
    }
    headers
}

pub fn note_response_headers(h: &Headers, kind: NoteResponseKind<'_>) -> Result<()> {
    for (key, value) in note_header_values(kind) {
        h.set(key, value)?;
    }
    Ok(())
}

const NOTE_HEADERS: [(&str, &str); 4] = [
    ("Cache-Control", "private, no-store"),
    ("X-Robots-Tag", "noindex"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
];

fn value<'a>(row: &'a turso::Row, key: &str) -> Option<&'a str> {
    row.get(key).and_then(|v| v.as_str())
}

fn not_found() -> Result<Response> {
    let h = Headers::new();
    note_response_headers(&h, NoteResponseKind::NotFound)?;
    Ok(Response::error("Not found", 404)?.with_headers(h))
}

pub async fn handle(
    req: Request,
    env: Env,
    path: &str,
    query: &HashMap<String, String>,
    is_owner: bool,
    _session_login: Option<&str>,
    lang: &str,
) -> Result<Response> {
    let Some(route) = parse_note_route(path) else {
        return not_found();
    };
    if matches!(
        decide_note_access(&route, is_owner, None, None),
        NoteAccess::NotFound
    ) && matches!(
        route,
        NoteRoute::OwnerList
            | NoteRoute::OwnerNew
            | NoteRoute::OwnerEdit(_)
            | NoteRoute::OwnerPost(_)
    ) {
        return not_found();
    }
    // Owner form routes arrive in T6. Keep them inside this isolated namespace.
    if matches!(
        route,
        NoteRoute::OwnerList
            | NoteRoute::OwnerNew
            | NoteRoute::OwnerEdit(_)
            | NoteRoute::OwnerPost(_)
    ) {
        return not_found();
    }
    if req.method() != Method::Get {
        return not_found();
    }
    let token = note_token(query);
    if !is_owner && !token.is_some_and(is_note_token) {
        return not_found();
    }
    let url = env.secret("TURSO_URL")?.to_string();
    let db_token = env.secret("TURSO_TOKEN")?.to_string();
    match &route {
        NoteRoute::SharePage(id) => {
            // Only the public column list is selected. The correlated EXISTS does not
            // expose purpose, author, or attachment to the rendering layer.
            let sql = if is_owner {
                format!(
                    "SELECT {NOTE_PUBLIC_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL"
                )
            } else {
                format!("SELECT {NOTE_PUBLIC_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL AND EXISTS (SELECT 1 FROM note_share_tokens WHERE note_id = notes.id AND token = ? AND status = 'active')")
            };
            let mut args = vec![turso::Arg::Text(id.clone())];
            if !is_owner {
                args.push(turso::Arg::Text(token.unwrap().into()));
            }
            let results =
                turso::pipeline_args(&url, &db_token, &[turso::Stmt { sql, args }]).await?;
            let Some(row) = results.first().and_then(|r| r.rows.first()) else {
                return not_found();
            };
            let access = AccessRow {
                token_note_id: Some(id.clone()),
                token_status: Some("active".into()),
                note_deleted: false,
                image_exists: false,
            };
            if decide_note_access(&route, is_owner, token, Some(&access)) != NoteAccess::Allow {
                return not_found();
            }
            let note = render::notes::PublicNote {
                id: id.clone(),
                title: value(row, "title").unwrap_or("").into(),
                body: value(row, "body").unwrap_or("").into(),
                place_name: value(row, "place_name").map(str::to_owned),
                taken_on: value(row, "taken_on").map(str::to_owned),
            };
            let images = turso::pipeline_args(&url, &db_token, &[turso::Stmt { sql: "SELECT image_id FROM note_images WHERE note_id = ? ORDER BY sort_order, image_id".into(), args: vec![turso::Arg::Text(id.clone())] }]).await?;
            let ids: Vec<String> = images
                .first()
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|r| value(r, "image_id").map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let html = render::notes::render_share_page(&note, &ids, token.unwrap_or(""), lang);
            let h = Headers::new();
            note_response_headers(&h, NoteResponseKind::SharePage)?;
            Ok(Response::from_html(html)?.with_headers(h))
        }
        NoteRoute::ShareImage(id, image) => {
            // One query checks note, token and image membership; the R2 key comes
            // solely from the returned image row.
            let results = turso::pipeline_args(&url, &db_token, &[turso::Stmt {
                sql: "SELECT n.deleted_at, i.r2_key, i.content_type, s.note_id AS token_note_id, s.status AS token_status FROM notes n LEFT JOIN note_images i ON i.note_id = n.id AND i.image_id = ? LEFT JOIN note_share_tokens s ON s.note_id = n.id AND s.token = ? WHERE n.id = ?".into(),
                args: vec![turso::Arg::Text(image.clone()), token.map(|s| turso::Arg::Text(s.into())).unwrap_or(turso::Arg::Null), turso::Arg::Text(id.clone())],
            }]).await?;
            let Some(row) = results.first().and_then(|r| r.rows.first()) else {
                return not_found();
            };
            let access = AccessRow {
                token_note_id: value(row, "token_note_id").map(str::to_owned),
                token_status: value(row, "token_status").map(str::to_owned),
                note_deleted: value(row, "deleted_at").is_some(),
                image_exists: value(row, "r2_key").is_some(),
            };
            if decide_note_access(&route, is_owner, token, Some(&access)) != NoteAccess::Allow {
                return not_found();
            }
            let Some(key) = value(row, "r2_key") else {
                return not_found();
            };
            let Some(content_type) = value(row, "content_type") else {
                return not_found();
            };
            if content_type != "image/jpeg" && content_type != "image/png" {
                return not_found();
            }
            let bucket = env.bucket("NOTES")?;
            let Some(object) = bucket.get(key).execute().await? else {
                return not_found();
            };
            let Some(body) = object.body() else {
                return not_found();
            };
            let h = Headers::new();
            note_response_headers(&h, NoteResponseKind::Image(content_type))?;
            Ok(Response::from_bytes(body.bytes().await?)?.with_headers(h))
        }
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    const ID: &str = "note_0123456789abcdef";
    const OTHER: &str = "note_1111111111111111";
    const IMG: &str = "0123456789abcdef";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    fn headers(kind: NoteResponseKind<'_>) -> BTreeMap<&'static str, String> {
        note_header_values(kind)
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect()
    }

    fn assert_private_headers(h: &BTreeMap<&str, String>) {
        assert_eq!(
            h.get("Cache-Control").map(String::as_str),
            Some("private, no-store")
        );
        assert_eq!(h.get("X-Robots-Tag").map(String::as_str), Some("noindex"));
        assert_eq!(
            h.get("Referrer-Policy").map(String::as_str),
            Some("no-referrer")
        );
        assert_eq!(
            h.get("X-Content-Type-Options").map(String::as_str),
            Some("nosniff")
        );
    }

    #[test]
    fn share_page_final_headers_include_html_content_type() {
        let h = headers(NoteResponseKind::SharePage);
        assert_private_headers(&h);
        assert_eq!(
            h.get("Content-Type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(h.len(), 5);
    }

    #[test]
    fn image_final_headers_use_db_content_type() {
        for content_type in ["image/jpeg", "image/png"] {
            let h = headers(NoteResponseKind::Image(content_type));
            assert_private_headers(&h);
            assert_eq!(
                h.get("Content-Type").map(String::as_str),
                Some(content_type)
            );
            assert_eq!(
                h.get("Content-Disposition").map(String::as_str),
                Some("inline")
            );
            assert_eq!(h.len(), 6);
        }
    }

    #[test]
    fn not_found_final_headers_are_private() {
        let h = headers(NoteResponseKind::NotFound);
        assert_private_headers(&h);
        assert_eq!(
            h.get("Content-Type").map(String::as_str),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(h.len(), 5);
    }
    fn row() -> AccessRow {
        AccessRow {
            token_note_id: Some(ID.into()),
            token_status: Some("active".into()),
            note_deleted: false,
            image_exists: true,
        }
    }

    #[test]
    fn uniform_404() {
        let page = NoteRoute::SharePage(ID.into());
        let img = NoteRoute::ShareImage(ID.into(), IMG.into());
        assert_eq!(
            decide_note_access(&page, false, None, Some(&row())),
            NoteAccess::NotFound
        );
        assert_eq!(
            decide_note_access(&page, false, Some("bad"), Some(&row())),
            NoteAccess::NotFound
        );
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), None),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.token_status = Some("inactive".into());
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.token_note_id = Some(OTHER.into());
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.note_deleted = true;
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.image_exists = false;
        assert_eq!(
            decide_note_access(&img, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        assert!(parse_note_route("/n/note_xxx").is_none());
    }

    #[test]
    fn owner_without_token_allowed() {
        assert_eq!(
            decide_note_access(&NoteRoute::SharePage(ID.into()), true, None, Some(&row())),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(
                &NoteRoute::ShareImage(ID.into(), IMG.into()),
                true,
                None,
                Some(&row())
            ),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(&NoteRoute::OwnerList, true, None, None),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(&NoteRoute::OwnerList, false, None, None),
            NoteAccess::NotFound
        );
    }

    #[test]
    fn plan_query_token_ignored() {
        let query = std::collections::HashMap::from([("token".to_string(), TOKEN.to_string())]);
        assert_eq!(note_token(&query), None);
        assert_eq!(
            decide_note_access(
                &NoteRoute::SharePage(ID.into()),
                false,
                note_token(&query),
                Some(&row())
            ),
            NoteAccess::NotFound
        );
    }

    #[test]
    fn parse_route_rejects_traversal() {
        assert!(parse_note_route("/n/../x").is_none());
        assert!(parse_note_route("/n/note_xxx/img/../../a").is_none());
        assert!(parse_note_route(&format!("/n/{ID}/img/{IMG}/more")).is_none());
        assert!(is_note_path("/notes"));
        assert!(is_note_path(&format!("/n/{ID}")));
        assert!(!is_note_path("/notesfoo"));
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn headers_set() {
        let h = Headers::new();
        note_response_headers(&h, NoteResponseKind::SharePage).unwrap();
        for (key, value) in [
            ("Cache-Control", "private, no-store"),
            ("X-Robots-Tag", "noindex"),
            ("Referrer-Policy", "no-referrer"),
            ("X-Content-Type-Options", "nosniff"),
        ] {
            assert_eq!(h.get(key).unwrap().as_deref(), Some(value));
        }
        assert_eq!(
            h.get("Content-Type").unwrap().as_deref(),
            Some("text/html; charset=utf-8")
        );
        note_response_headers(&h, NoteResponseKind::Image("image/jpeg")).unwrap();
        assert_eq!(
            h.get("Content-Type").unwrap().as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(
            h.get("Content-Disposition").unwrap().as_deref(),
            Some("inline")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn headers_set() {
        assert_eq!(
            NOTE_HEADERS,
            [
                ("Cache-Control", "private, no-store"),
                ("X-Robots-Tag", "noindex"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff")
            ]
        );
    }
}
