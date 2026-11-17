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

pub fn owner_shell(title: &str, body: &str, lang: &str) -> String {
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!("<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"google\" content=\"notranslate\"><meta name=\"robots\" content=\"noindex\"><title>{}</title><style>{}</style></head><body><main>{body}</main></body></html>", super::esc(title), crate::styles::CSS)
}

pub fn render_owner_list(notes: &[(String, String, String)], lang: &str) -> String {
    let mut body = format!("<h1>{}</h1><p><a href=\"/notes/new\">{}</a></p><ul>", crate::i18n::t("notesTitle", lang), crate::i18n::t("noteNew", lang));
    for (id, title, purpose) in notes {
        if note_core::is_note_id(id) {
            let tail = if purpose.trim().is_empty() { String::new() } else { format!(" — {}", super::esc(purpose)) };
            body.push_str(&format!("<li><a href=\"/notes/{id}/edit\">{}</a>{tail}</li>", super::esc(title)));
        }
    }
    body.push_str("</ul>");
    owner_shell(crate::i18n::t("notesTitle", lang), &body, lang)
}

pub fn render_owner_form(form: &crate::notes::NoteForm, csrf: &str, delete_csrf: Option<&str>, error: Option<&str>, lang: &str) -> String {
    let new = form.id.is_empty();
    let action = if new { "/notes/new".into() } else { format!("/notes/{}/edit", form.id) };
    let mut body = format!("<h1>{}</h1><p><a href=\"/notes\">{}</a></p>", if new { crate::i18n::t("noteNew", lang) } else { crate::i18n::t("noteEdit", lang) }, crate::i18n::t("notesTitle", lang));
    if let Some(key) = error { body.push_str(&format!("<p role=\"alert\">{}</p>", crate::i18n::t(key, lang))); }
    body.push_str(&format!("<form method=\"post\" action=\"{action}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"form_nonce\" value=\"{}\">", super::esc(csrf), super::esc(&form.form_nonce)));
    for (name, label, val) in [
        ("title", "noteTitle", &form.title), ("purpose", "notePurpose", &form.purpose),
        ("place_name", "notePlace", &form.place_name), ("taken_on", "noteDate", &form.taken_on),
        ("attached_plan_id", "notePlan", &form.attached_plan_id),
        ("attached_destination", "noteDestination", &form.attached_destination),
        ("attached_day_number", "noteDay", &form.attached_day_number)
    ] {
        let typ = if name == "taken_on" { "date" } else if name == "attached_day_number" { "number" } else { "text" };
        body.push_str(&format!("<p><label>{}<br><input type=\"{typ}\" name=\"{name}\" value=\"{}\"></label></p>", crate::i18n::t(label, lang), super::esc(val)));
    }
    body.push_str(&format!("<p><label>{}<br><textarea name=\"body\" rows=\"12\">{}</textarea></label></p><button type=\"submit\">{}</button></form>", crate::i18n::t("noteBody", lang), super::esc(&form.body), crate::i18n::t("noteSave", lang)));
    if let Some(csrf) = delete_csrf {
        body.push_str(&format!("<form method=\"post\" action=\"/notes/{}/delete\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{}</button></form>", form.id, super::esc(csrf), crate::i18n::t("noteDelete", lang)));
    }
    owner_shell(crate::i18n::t("noteEdit", lang), &body, lang)
}

pub fn render_expired_refill(title: &str, body: &str, lang: &str) -> String {
    let content = format!("<h1>{}</h1><p>{}</p><p><a href=\"/auth/login\">{}</a></p><label>{}<textarea readonly rows=\"2\">{}</textarea></label><label>{}<textarea readonly rows=\"12\">{}</textarea></label>",
        crate::i18n::t("noteExpired", lang), crate::i18n::t("noteExpiredHelp", lang), crate::i18n::t("noteLogin", lang), crate::i18n::t("noteTitle", lang), super::esc(title), crate::i18n::t("noteBody", lang), super::esc(body));
    owner_shell(crate::i18n::t("noteExpired", lang), &content, lang)
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

    #[test]
    fn owner_shell_is_notranslate_and_list_omits_empty_purpose_dash() {
        let html = owner_shell("t", "b", "zh");
        assert!(html.contains("content=\"notranslate\""));
        assert!(html.contains("lang=\"zh-TW\""));
        let id = note_core::note_id_from_bytes([1, 2, 3, 4, 5, 6, 7, 8]);
        let list = render_owner_list(&[(id.clone(), "A".into(), "".into()), (id, "B".into(), "p".into())], "zh");
        assert!(!list.contains("</a> — </li>"), "{list}");
        assert!(list.contains("</a> — p</li>"), "{list}");
    }
}
