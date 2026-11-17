pub struct PublicNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub place_name: Option<String>,
    pub taken_on: Option<String>,
}

/// The note body BOTH the share page and the owner's share preview render —
/// extracted so the preview is byte-identical to what the recipient sees
/// (spec G4: 分享前預覽確認). `token` is the share token ("" → no `?t=` suffix;
/// the owner's own session authorizes the image route without one).
pub fn share_article(note: &PublicNote, images: &[String], token: &str, lang: &str) -> String {
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
    body
}

pub fn render_share_page(note: &PublicNote, images: &[String], token: &str, lang: &str) -> String {
    let title = super::esc(&note.title);
    let body = share_article(note, images, token, lang);
    // Keep the page self-contained and script-free; shared body text is never linkified.
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!("<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"google\" content=\"notranslate\"><meta name=\"robots\" content=\"noindex\"><title>{title}</title><style>{}</style></head><body>{body}</body></html>", crate::styles::CSS)
}

pub fn owner_shell(title: &str, body: &str, lang: &str) -> String {
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!("<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"google\" content=\"notranslate\"><meta name=\"robots\" content=\"noindex\"><title>{}</title><style>{}</style></head><body><main>{body}</main></body></html>", super::esc(title), crate::styles::CSS)
}

pub fn render_owner_list(notes: &[(String, String, String)], csrf_stop_all: &str, lang: &str) -> String {
    let mut body = format!("<h1>{}</h1><p><a href=\"/notes/new\">{}</a></p><ul>", crate::i18n::t("notesTitle", lang), crate::i18n::t("noteNew", lang));
    for (id, title, purpose) in notes {
        if note_core::is_note_id(id) {
            let tail = if purpose.trim().is_empty() { String::new() } else { format!(" — {}", super::esc(purpose)) };
            body.push_str(&format!("<li><a href=\"/notes/{id}/edit\">{}</a>{tail}</li>", super::esc(title)));
        }
    }
    body.push_str("</ul>");
    // One-click revoke of EVERY note's share tokens (Yang §12 Q6), CSRF-scoped
    // to the session; a plain form, no JS.
    body.push_str(&format!(
        "<form method=\"post\" action=\"/notes/share-stop-all\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{}</button></form>",
        super::esc(csrf_stop_all),
        crate::i18n::t("noteShareStopAll", lang)
    ));
    owner_shell(crate::i18n::t("notesTitle", lang), &body, lang)
}

/// The text form alone (no shell) — shared by the new-note page, the refill
/// pages and the full edit page so every surface posts the same fields.
fn owner_form_body(form: &crate::notes::NoteForm, csrf: &str, delete_csrf: Option<&str>, error: Option<&str>, lang: &str) -> String {
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
    body
}

pub fn render_owner_form(form: &crate::notes::NoteForm, csrf: &str, delete_csrf: Option<&str>, error: Option<&str>, lang: &str) -> String {
    let body = owner_form_body(form, csrf, delete_csrf, error, lang);
    owner_shell(if form.id.is_empty() { crate::i18n::t("noteNew", lang) } else { crate::i18n::t("noteEdit", lang) }, &body, lang)
}

/// The full edit page: text form + photo management + share controls.
pub fn render_owner_edit(
    form: &crate::notes::NoteForm,
    csrf: &str,
    delete_csrf: Option<&str>,
    images_html: &str,
    share_html: &str,
    error: Option<&str>,
    lang: &str,
) -> String {
    let body = format!("{}{images_html}{share_html}", owner_form_body(form, csrf, delete_csrf, error, lang));
    owner_shell(crate::i18n::t("noteEdit", lang), &body, lang)
}

pub fn render_expired_refill(title: &str, body: &str, lang: &str) -> String {
    let content = format!("<h1>{}</h1><p>{}</p><p><a href=\"/auth/login\">{}</a></p><label>{}<textarea readonly rows=\"2\">{}</textarea></label><label>{}<textarea readonly rows=\"12\">{}</textarea></label>",
        crate::i18n::t("noteExpired", lang), crate::i18n::t("noteExpiredHelp", lang), crate::i18n::t("noteLogin", lang), crate::i18n::t("noteTitle", lang), super::esc(title), crate::i18n::t("noteBody", lang), super::esc(body));
    owner_shell(crate::i18n::t("noteExpired", lang), &content, lang)
}

/// Owner error page for a rejected POST whose body was never read (missing /
/// over-cap Content-Length) or whose photo bytes were over the limit: the
/// ZH/EN message plus a link back to the note — the browser's back button was
/// previously the only way out of the bare-English 413/400. `back_href` is
/// built by the caller from validated note ids; `esc` keeps the attribute
/// closed regardless.
pub fn render_owner_error(back_href: &str, error_key: &str, lang: &str) -> String {
    let t = crate::i18n::t;
    let content = format!(
        "<h1>{}</h1><p role=\"alert\">{}</p><p><a href=\"{}\">{}</a></p>",
        t("noteRequestRejected", lang),
        t(error_key, lang),
        super::esc(back_href),
        t("noteBack", lang),
    );
    owner_shell(t("noteRequestRejected", lang), &content, lang)
}

// ── T7: photo upload/delete/reorder, share flow, plan-page entry ──

/// One image's action buttons with their (precomputed, action-scoped) CSRF
/// tokens — kept as data so the render fn stays pure and testable.
pub struct ImageUi {
    pub image_id: String,
    pub csrf_up: String,
    pub csrf_down: String,
    pub csrf_delete: String,
}

/// The photo section of the edit page: thumbnails through the authenticated
/// owner image route, per-image 上移／下移／刪除 buttons, and the one-at-a-time
/// upload form. The file input MUST carry `accept="image/jpeg,image/png"` —
/// iPhone Safari converts HEIC to JPEG only when the accept attribute narrows
/// it (spec §8.2, feasibility key for phone uploads); without it phones
/// effectively cannot upload.
pub fn render_owner_images(note_id: &str, images: &[ImageUi], csrf_add: &str, lang: &str) -> String {
    let t = crate::i18n::t;
    let mut body = format!("<h2>{}</h2>", t("notePhotos", lang));
    if images.is_empty() {
        body.push_str(&format!("<p>{}</p>", t("noteNoPhotos", lang)));
    }
    for img in images {
        if !note_core::is_image_id(&img.image_id) {
            continue;
        }
        body.push_str(&format!(
            "<figure><img loading=\"lazy\" src=\"/n/{note_id}/img/{}\" alt=\"{}\"></figure>",
            img.image_id,
            t("notePhoto", lang)
        ));
        for (op, csrf_tok, label) in [
            ("up", &img.csrf_up, t("noteMoveUp", lang)),
            ("down", &img.csrf_down, t("noteMoveDown", lang)),
            ("delete", &img.csrf_delete, t("noteDeletePhoto", lang)),
        ] {
            body.push_str(&format!(
                "<form method=\"post\" action=\"/notes/{note_id}/image/{}/{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{label}</button></form>",
                img.image_id, op, super::esc(csrf_tok)
            ));
        }
    }
    // One photo per POST (spec §8.2): repeat the form to add more.
    body.push_str(&format!(
        "<form method=\"post\" action=\"/notes/{note_id}/image\" enctype=\"multipart/form-data\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><p><input type=\"file\" name=\"image\" accept=\"image/jpeg,image/png\"></p><button type=\"submit\">{}</button></form>",
        super::esc(csrf_add),
        t("noteAddPhoto", lang)
    ));
    body
}

/// The share controls of the edit page: active-link count, preview button,
/// and — when links exist — the revoke button.
pub fn render_owner_share(note_id: &str, active_count: u64, csrf_preview: &str, csrf_deactivate: &str, lang: &str) -> String {
    let t = crate::i18n::t;
    let mut body = format!(
        "<h2>{}</h2><p>{}: {}</p>",
        t("noteShare", lang),
        t("noteShareLinks", lang),
        active_count
    );
    body.push_str(&format!(
        "<form method=\"post\" action=\"/notes/{note_id}/share-preview\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{}</button></form>",
        super::esc(csrf_preview),
        t("noteSharePreview", lang)
    ));
    if active_count > 0 {
        body.push_str(&format!(
            "<form method=\"post\" action=\"/notes/{note_id}/share-deactivate\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{}</button></form>",
            super::esc(csrf_deactivate),
            t("noteShareStop", lang)
        ));
    }
    body
}

/// 分享預覽: the recipient's exact page (the SAME `share_article` the share
/// route renders) plus a confirm button that POSTs share-create. No token
/// exists yet at preview time — the owner's session authorizes the image URLs.
pub fn render_share_preview(note: &PublicNote, images: &[String], csrf_create: &str, lang: &str) -> String {
    let t = crate::i18n::t;
    let article = share_article(note, images, "", lang);
    let content = format!(
        "<h1>{}</h1><p>{}</p>{article}\
         <form method=\"post\" action=\"/notes/{}/share-create\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">{}</button></form>\
         <p><a href=\"/notes/{}/edit\">{}</a></p>",
        t("noteSharePreview", lang),
        t("noteSharePreviewHelp", lang),
        note.id,
        super::esc(csrf_create),
        t("noteShareCreate", lang),
        note.id,
        t("noteShareBack", lang),
    );
    owner_shell(t("noteSharePreview", lang), &content, lang)
}

/// After share-create: the copyable link in a plain readonly input (no JS).
pub fn render_share_created(link: &str, lang: &str) -> String {
    let t = crate::i18n::t;
    let content = format!(
        "<h1>{}</h1><p>{}</p><p><label>{}<br><input type=\"url\" readonly value=\"{}\"></label></p><p><a href=\"/notes\">{}</a></p>",
        t("noteShareLink", lang),
        t("noteShareLinkHelp", lang),
        t("noteShareLink", lang),
        super::esc(link),
        t("notesTitle", lang),
    );
    owner_shell(t("noteShareLink", lang), &content, lang)
}

/// The owner-only per-day note entry on the plan page (spec §8.1, Yang §10 Q3):
/// rendered only for the owner view — a viewer's page never contains it.
pub fn owner_day_notes_block(notes: &[(String, String)], lang: &str) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut body = format!("<div class=\"day-notes\"><h3>{}</h3><ul>", crate::i18n::t("noteDayNotes", lang));
    for (id, title) in notes {
        if note_core::is_note_id(id) {
            body.push_str(&format!("<li><a href=\"/notes/{id}/edit\">{}</a></li>", super::esc(title)));
        }
    }
    body.push_str("</ul></div>");
    body
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
        let list = render_owner_list(&[(id.clone(), "A".into(), "".into()), (id, "B".into(), "p".into())], "stop-all-csrf", "zh");
        assert!(!list.contains("</a> — </li>"), "{list}");
        assert!(list.contains("</a> — p</li>"), "{list}");
        // One-click stop for every note's share tokens, CSRF-scoped, no JS.
        assert!(list.contains("action=\"/notes/share-stop-all\""));
        assert!(list.contains("value=\"stop-all-csrf\""));
    }

    // ── T7: photo upload/delete/reorder, share flow, plan-page entry ──

    #[test]
    fn share_preview_equals_share_page() {
        let note = sample();
        let imgs = vec!["0123456789abcdef".to_string()];
        // The preview's note body is byte-identical to the share page's — the
        // owner confirms exactly what the recipient will see (spec G4).
        let article = share_article(&note, &imgs, "", "zh");
        let page = render_share_page(&note, &imgs, "", "zh");
        assert!(page.contains(&article), "share page must render the shared article");
        let preview = render_share_preview(&note, &imgs, "csrf-create-tok", "zh");
        assert!(preview.contains(&article), "preview must render the SAME article:\n{preview}");
        // The confirm button posts to share-create carrying the CSRF; no script.
        assert!(preview.contains(&format!("action=\"/notes/{}/share-create\"", note.id)));
        assert!(preview.contains("value=\"csrf-create-tok\""));
        assert!(!preview.contains("<script"));
    }

    #[test]
    fn viewer_plan_page_has_no_notes_block() {
        let plan = crate::model::Plan {
            plan_id: "kyoto-2026".into(),
            days: vec![crate::model::Day { day_number: 1, ..Default::default() }],
            ..Default::default()
        };
        let notes = std::collections::BTreeMap::from([(
            1i64,
            vec![("note_0123456789abcdef".to_string(), "GODIVA".to_string())],
        )]);
        // Viewer (empty owner chrome): the day-notes data never renders, even if
        // the caller passed Some — the block is owner-chrome-gated in the render.
        let viewer = crate::render::render_plan_at_with_notes(
            &plan, "zh", None, &crate::render::map::MapStatus::default(), "", None, Some(&notes), 0,
        );
        assert!(!viewer.contains("day-notes"), "{viewer}");
        // Owner: the block renders at that day, linking to the note editor.
        let owner = crate::render::render_plan_at_with_notes(
            &plan, "zh", None, &crate::render::map::MapStatus::default(), "owner-chrome", None, Some(&notes), 0,
        );
        assert!(owner.contains("class=\"day-notes\""), "{owner}");
        assert!(owner.contains("href=\"/notes/note_0123456789abcdef/edit\""));
    }

    #[test]
    fn owner_day_notes_block_escapes_and_links() {
        let html = owner_day_notes_block(&[("note_0123456789abcdef".to_string(), "<script>".to_string())], "zh");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("href=\"/notes/note_0123456789abcdef/edit\""));
        assert!(html.contains(crate::i18n::t("noteDayNotes", "zh")));
    }

    #[test]
    fn upload_input_accepts_jpeg_png_only() {
        // iPhone Safari converts HEIC to JPEG ONLY when the input carries the
        // accept attribute — without it phones effectively cannot upload.
        let html = render_owner_images(
            "note_0123456789abcdef",
            &[ImageUi { image_id: "0123456789abcdef".into(), csrf_up: "tok-up".into(), csrf_down: "tok-down".into(), csrf_delete: "tok-del".into() }],
            "tok-add",
            "zh",
        );
        assert!(html.contains("accept=\"image/jpeg,image/png\""), "{html}");
        assert!(html.contains("enctype=\"multipart/form-data\""));
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/image\""));
        // Per-image controls, each with its own action-scoped CSRF.
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/image/0123456789abcdef/up\""));
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/image/0123456789abcdef/down\""));
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/image/0123456789abcdef/delete\""));
        assert!(html.contains("value=\"tok-up\""));
        assert!(html.contains("value=\"tok-del\""));
        assert!(html.contains("value=\"tok-add\""));
        // The image itself loads through the authenticated owner route.
        assert!(html.contains("src=\"/n/note_0123456789abcdef/img/0123456789abcdef\""));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn share_created_page_shows_copyable_link_without_js() {
        let link = "https://travel.example/n/note_0123456789abcdef?t=0123456789abcdef0123456789abcdef";
        let html = render_share_created(link, "zh");
        // A plain readonly input — copyable by hand, no JS.
        assert!(html.contains("readonly"));
        assert!(html.contains(&format!("value=\"{link}\"")));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn owner_share_section_lists_active_links_and_controls() {
        let html = render_owner_share("note_0123456789abcdef", 2, "tok-preview", "tok-stop", "zh");
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/share-preview\""));
        assert!(html.contains("action=\"/notes/note_0123456789abcdef/share-deactivate\""));
        assert!(html.contains("value=\"tok-preview\""));
        assert!(html.contains("value=\"tok-stop\""));
        assert!(html.contains(": 2"), "active count must render: {html}");
        assert!(!html.contains("<script"));
    }

    #[test]
    fn owner_error_page_renders_i18n_message_and_escaped_back_link() {
        // Over-limit / invalid-request rejections render as an owner error page
        // in the request language — with a way back, not bare English text.
        for lang in ["zh", "en"] {
            let html = render_owner_error("\" onmouseover=\"alert(1)", "notePhotoTooLarge", lang);
            assert!(html.contains(crate::i18n::t("noteRequestRejected", lang)), "{lang}: title missing");
            assert!(html.contains(crate::i18n::t("notePhotoTooLarge", lang)), "{lang}: message missing");
            assert!(html.contains(crate::i18n::t("noteBack", lang)), "{lang}: back label missing");
            // The back link is an escaped attribute — the raw href cannot break out.
            assert!(!html.contains("\" onmouseover=\"alert(1)"), "{lang}: raw href leaked");
            assert!(html.contains("&quot; onmouseover=&quot;alert(1)"), "{lang}: href must be escaped");
            assert!(html.contains("<a href=\""));
            assert!(!html.contains("<script"));
        }
    }
}
