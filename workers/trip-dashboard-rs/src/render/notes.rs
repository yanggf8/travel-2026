pub struct PublicNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub place_name: Option<String>,
    pub taken_on: Option<String>,
}

pub fn render_share_page(note: &PublicNote, images: &[String], token: &str, lang: &str) -> String {
    let title = super::esc(&note.title);
    let token_suffix = if note_core::is_note_token(token) {
        format!("?t={token}")
    } else {
        String::new()
    };
    let mut body = format!("<main><article><h1>{title}</h1>");
    for image in images {
        if note_core::is_image_id(image) {
            body.push_str(&format!("<figure><img loading=\"lazy\" src=\"/n/{}/img/{}{token_suffix}\" alt=\"{}\"></figure>", note.id, image, crate::i18n::t("notePhoto", lang)));
        }
    }
    if let Some(place) = &note.place_name {
        body.push_str(&format!(
            "<p>{}: {}</p>",
            crate::i18n::t("notePlace", lang),
            super::esc(place)
        ));
    }
    if let Some(date) = &note.taken_on {
        if let Some(day) = date.get(..10) {
            body.push_str(&format!(
                "<p>{}: {}</p>",
                crate::i18n::t("noteDate", lang),
                super::esc(day)
            ));
        }
    }
    body.push_str(&format!(
        "<div style=\"white-space:pre-wrap\">{}</div></article></main>",
        super::esc(&note.body)
    ));
    // Keep the page self-contained and script-free; shared body text is never linkified.
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!("<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"google\" content=\"notranslate\"><meta name=\"robots\" content=\"noindex\"><title>{title}</title><style>{}</style></head><body>{body}</body></html>", crate::styles::CSS)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> PublicNote {
        PublicNote {
            id: "note_0123456789abcdef".into(),
            title: "<script>".into(),
            body: "See https://x 😀 <script>".into(),
            place_name: Some("台北".into()),
            taken_on: Some("2026-11-17 10:30".into()),
        }
    }
    #[test]
    fn share_page_no_purpose_no_created_by() {
        let html = render_share_page(&sample(), &[], "0123456789abcdef0123456789abcdef", "zh");
        assert!(!html.contains("private-purpose"));
        assert!(!html.contains("yanggf8"));
    }
    #[test]
    fn share_page_escapes_and_no_autolink() {
        let html = render_share_page(&sample(), &[], "0123456789abcdef0123456789abcdef", "zh");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("https://x 😀"));
        assert!(!html.contains("<a "));
        assert!(!html.contains("10:30"));
    }
    #[test]
    fn share_page_noindex_meta() {
        let html = render_share_page(&sample(), &[], "0123456789abcdef0123456789abcdef", "zh");
        assert!(html.contains("<meta name=\"robots\" content=\"noindex\">"));
    }
    #[test]
    fn owner_query_token_cannot_inject_image_attribute() {
        let html = render_share_page(
            &sample(),
            &["0123456789abcdef".into()],
            "\" onerror=\"alert(1)",
            "zh",
        );
        assert!(!html.contains("onerror"));
        assert!(html.contains("/n/note_0123456789abcdef/img/0123456789abcdef\""));
    }
}
