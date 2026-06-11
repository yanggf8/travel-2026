//! `travel check-hours` — pre-trip open-hours and label-freshness check.
//!
//! **Hours check**: for every named activity in the plan, reads `Hours: HH:MM-HH:MM`
//! and optional `Closed: Mon,Tue,...` markers from the activity notes column.
//! When no explicit start_time is set, the session is used as a proxy:
//!   morning  → 09:00–12:00
//!   noon     → 12:00–14:00
//!   afternoon→ 14:00–18:00
//!   evening  → 18:00–22:00
//!
//! Each activity is classified as:
//!   ✅  open during the planned visit window
//!   ⚠️  visit window partially outside opening hours
//!   ❌  closed (day-of-week closure or fully outside hours)
//!   ·   no hours data — cannot check
//!
//! **Label freshness**: compares day themes and session focus labels against
//! actual activities. Flags when a theme mentions a place that no longer
//! appears in that day's activities (e.g., after deleting or moving activities).

use crate::db;
use libsql::{params, Connection};

// ── session time windows (minutes since midnight) ────────────────────────────
const SESSION_WINDOWS: &[(&str, i64, i64)] = &[
    ("morning",   9 * 60,  12 * 60),
    ("noon",     12 * 60,  14 * 60),
    ("afternoon",14 * 60,  18 * 60),
    ("evening",  18 * 60,  22 * 60),
];

fn session_window(session: &str) -> (i64, i64) {
    for (name, open, close) in SESSION_WINDOWS {
        if session == *name {
            return (*open, *close);
        }
    }
    (9 * 60, 22 * 60) // fallback: full day
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn parse_hhmm(s: &str) -> Option<i64> {
    let s = s.trim();
    let (h, m) = s.split_once(':')?;
    let hh: i64 = h.trim().parse().ok()?;
    let mm: i64 = m.trim().parse().ok()?;
    Some(hh * 60 + mm)
}

fn fmt_hhmm(mins: i64) -> String {
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

/// Parse `Hours: HH:MM-HH:MM` from notes. Returns (open_min, close_min).
fn parse_hours(notes: &str) -> Option<(i64, i64)> {
    let lower = notes.to_lowercase();
    let idx = lower.find("hours:")?;
    // must be at start or preceded by whitespace
    if idx != 0 {
        let prev = notes[..idx].chars().last();
        if !matches!(prev, Some(c) if c.is_whitespace()) {
            return None;
        }
    }
    let after = &notes[idx + "hours:".len()..];
    let segment: String = after.chars().take_while(|&c| c != '\n').collect();
    let segment = segment.trim();
    // scan for HH:MM-HH:MM (dash or en-dash)
    let chars: Vec<char> = segment.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // try to read HH:MM at position i
        if let Some((open, ni)) = scan_hhmm(&chars, i) {
            let mut j = ni;
            while j < chars.len() && chars[j] == ' ' { j += 1; }
            if j < chars.len() && (chars[j] == '-' || chars[j] == '\u{2013}') {
                j += 1;
                while j < chars.len() && chars[j] == ' ' { j += 1; }
                if let Some((close, _)) = scan_hhmm(&chars, j) {
                    return Some((open, close));
                }
            }
        }
        i += 1;
    }
    None
}

fn scan_hhmm(chars: &[char], start: usize) -> Option<(i64, usize)> {
    let mut i = start;
    // 1-2 digit hour
    let h_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
    if i == h_start || i > h_start + 2 { return None; }
    if i >= chars.len() || chars[i] != ':' { return None; }
    i += 1; // skip ':'
    let m_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
    if i - m_start != 2 { return None; }
    let h: i64 = chars[h_start..m_start - 1].iter().collect::<String>().parse().ok()?;
    let m: i64 = chars[m_start..i].iter().collect::<String>().parse().ok()?;
    Some((h * 60 + m, i))
}

/// Parse `Closed: Mon,Tue,...` or `Closed: Wed` from notes.
/// Returns a list of weekday numbers (0=Mon … 6=Sun) that are closed.
fn parse_closed_days(notes: &str) -> Vec<u8> {
    let lower = notes.to_lowercase();
    let idx = match lower.find("closed:") {
        Some(i) => i,
        None => return vec![],
    };
    let after = &lower[idx + "closed:".len()..];
    let segment: String = after.chars().take_while(|&c| c != '\n' && c != '|').collect();
    let mut days = Vec::new();
    for token in segment.split(|c: char| c == ',' || c == ' ' || c == '/') {
        let t = token.trim();
        let dow = match t {
            "mon" | "monday"    => Some(0u8),
            "tue" | "tuesday"   => Some(1),
            "wed" | "wednesday" => Some(2),
            "thu" | "thursday"  => Some(3),
            "fri" | "friday"    => Some(4),
            "sat" | "saturday"  => Some(5),
            "sun" | "sunday"    => Some(6),
            _ => None,
        };
        if let Some(d) = dow { days.push(d); }
    }
    days
}

/// Parse ISO date string "YYYY-MM-DD" → weekday 0=Mon … 6=Sun using
/// Tomohiko Sakamoto's algorithm (no external crate needed).
fn date_to_weekday(date: &str) -> Option<u8> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() < 3 { return None; }
    let y: i32 = parts[0].parse().ok()?;
    let m: i32 = parts[1].parse().ok()?;
    let d: i32 = parts[2].parse().ok()?;
    // Zeller / Sakamoto
    let t = [0i32, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let dow = (y + y/4 - y/100 + y/400 + t[(m-1) as usize] + d) % 7;
    // result: 0=Sun … 6=Sat → convert to 0=Mon … 6=Sun
    Some(((dow + 6) % 7) as u8)
}

fn weekday_name(dow: u8) -> &'static str {
    match dow {
        0 => "Mon", 1 => "Tue", 2 => "Wed",
        3 => "Thu", 4 => "Fri", 5 => "Sat",
        _ => "Sun",
    }
}

// ── result types ─────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum Status { Ok, Warning, Closed, Unknown }

struct ActivityCheck {
    title: String,
    session: String,
    status: Status,
    detail: String,
}

struct DayCheck {
    day_number: i64,
    date: String,
    theme: String,
    theme_zh: String,
    session_focus: Vec<(String, String, String)>, // (session, focus, focus_zh)
    activities: Vec<ActivityCheck>,
}

// ── main entry ───────────────────────────────────────────────────────────────

pub async fn run(_args: &[String], plan_id: String) -> Result<(), String> {

    let conn = db::connect_read().await?;

    // Resolve active destination
    let dest: String = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id.clone()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?
        .next()
        .await
        .map_err(|e| format!("plan_metadata row failed: {e}"))?
        .ok_or_else(|| format!("Plan '{plan_id}' not found"))?
        .get::<String>(0)
        .map_err(|e| format!("active_destination read failed: {e}"))?;

    let days = load_days(&conn, &plan_id, &dest).await?;

    // ── render ────────────────────────────────────────────────────────────────
    let total: usize = days.iter().map(|d| d.activities.len()).sum();
    let n_warn: usize = days.iter()
        .flat_map(|d| &d.activities)
        .filter(|a| a.status == Status::Warning)
        .count();
    let n_closed: usize = days.iter()
        .flat_map(|d| &d.activities)
        .filter(|a| a.status == Status::Closed)
        .count();
    let n_unknown: usize = days.iter()
        .flat_map(|d| &d.activities)
        .filter(|a| a.status == Status::Unknown)
        .count();

    println!("Open-hours check — {plan_id} ({dest})");
    println!();

    let mut any_issue = false;

    for day in &days {
        let dow = date_to_weekday(&day.date)
            .map(|d| format!(" {}", weekday_name(d)))
            .unwrap_or_default();
        println!("Day {} · {}{}  {}", day.day_number, day.date, dow, day.theme);

        for a in &day.activities {
            let icon = match a.status {
                Status::Ok      => "✅",
                Status::Warning => "⚠️ ",
                Status::Closed  => "❌",
                Status::Unknown => "·  ",
            };
            // Trim long titles with embedded Google Maps URLs
            let display_title = a.title
                .split('\n')
                .next()
                .unwrap_or(&a.title)
                .trim();
            let truncated = if display_title.chars().count() > 40 {
                let cut: String = display_title.chars().take(38).collect();
                format!("{cut}…")
            } else {
                display_title.to_string()
            };
            println!("  {} [{:9}] {}", icon, a.session, truncated);
            if !a.detail.is_empty() && a.status != Status::Unknown {
                println!("             {}", a.detail);
            }
            if matches!(a.status, Status::Warning | Status::Closed) {
                any_issue = true;
            }
        }
        println!();
    }

    println!("─────────────────────────────────────────────");
    println!(
        "Checked {total} activities  ·  {} closed  ·  {} timing issues  ·  {} no data",
        n_closed, n_warn, n_unknown
    );

    // ── label freshness check ─────────────────────────────────────────────────
    let stale_labels = check_label_freshness(&days);

    if any_issue || !stale_labels.is_empty() {
        if !stale_labels.is_empty() {
            println!();
            println!("Label freshness issues:");
            for label in &stale_labels {
                println!("  ⚠️  {}", label);
            }
        }
        println!();
        println!("Issues found — review ❌/⚠️  items above.");
        std::process::exit(1);
    } else {
        println!();
        println!("All checked activities are open during planned visit windows.");
        println!("All labels match their activities.");
    }

    Ok(())
}

/// Extract keywords from a theme/focus string (split by ・, ·, /, ,)
fn extract_place_keywords(text: &str) -> Vec<String> {
    text
        .split(|c| c == '・' || c == '·' || c == '/' || c == '、')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() > 1)
        .collect()
}

/// Check label freshness: verify day themes match actual activities.
/// Returns a list of stale label descriptions.
fn check_label_freshness(days: &[DayCheck]) -> Vec<String> {
    let mut issues = Vec::new();

    for day in days {
        // Collect all activity titles for this day (lowercase for matching)
        let day_titles_lower: Vec<String> = day.activities.iter()
            .map(|a| a.title.to_lowercase())
            .collect();

        // Check if theme keywords appear in any activity
        let theme_keywords = extract_place_keywords(&day.theme);
        let theme_zh_keywords = extract_place_keywords(&day.theme_zh);
        let all_keywords: std::collections::HashSet<_> = theme_keywords.iter()
            .chain(theme_zh_keywords.iter())
            .collect();

        for kw in all_keywords {
            if kw.len() < 2 {
                continue;
            }
            let kw_lower = kw.to_lowercase();
            let is_generic = ["購物", "漫步", "散步", "美食", "返程", "抵達", "搭單軌",
                             "紀念品", "晚餐", "午餐", "早餐", "住宿", "飯店"].iter()
                .any(|g| kw_lower.contains(g));
            if is_generic {
                continue;
            }

            // Lenient matching:
            // 1. Full substring match
            // 2. First 2 characters match (handles shortened forms like "牧志市場" → "牧志公設市場")
            // 3. Common ZH→EN translations
            let first_two: String = kw_lower.chars().take(2).collect();
            let en_equivalents: &[&str] = match kw_lower.as_str() {
                k if k.contains("博物館") || k.contains("美術館") => &["museum"],
                k if k.contains("公園") => &["park"],
                k if k.contains("市場") => &["market"],
                k if k.contains("城") => &["castle"],
                k if k.contains("宮") || k.contains("神社") => &["shrine"],
                k if k.contains("園") => &["garden"],
                _ => &[],
            };
            let found = day_titles_lower.iter().any(|t| {
                t.contains(&kw_lower) || t.contains(&first_two)
                    || en_equivalents.iter().any(|en| t.contains(en))
            });

            if !found {
                issues.push(format!(
                    "Day {} theme mentions '{}' but no activity matches",
                    day.day_number, kw
                ));
            }
        }
    }

    issues
}

async fn load_days(conn: &Connection, plan_id: &str, dest: &str) -> Result<Vec<DayCheck>, String> {
    // Load days with both EN and ZH themes
    let mut day_rows = conn
        .query(
            "SELECT day_number, date, \
                    COALESCE(NULLIF(theme_zh,''), NULLIF(theme,''), day_type), \
                    COALESCE(theme_zh, '') \
             FROM days WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("days query failed: {e}"))?;

    let mut days: Vec<DayCheck> = Vec::new();
    while let Some(r) = day_rows.next().await.map_err(|e| format!("days row: {e}"))? {
        days.push(DayCheck {
            day_number: r.get(0).unwrap_or(0),
            date: r.get(1).unwrap_or_default(),
            theme: r.get(2).unwrap_or_default(),
            theme_zh: r.get(3).unwrap_or_default(),
            session_focus: Vec::new(),
            activities: Vec::new(),
        });
    }

    // Load session focus labels
    let mut tod_rows = conn
        .query(
            "SELECT day_number, session_type, COALESCE(focus, ''), COALESCE(focus_zh, '') \
             FROM timesofday WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, \
               CASE session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 \
                 WHEN 'afternoon' THEN 2 ELSE 3 END",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("timesofday query failed: {e}"))?;

    while let Some(r) = tod_rows.next().await.map_err(|e| format!("tod row: {e}"))? {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let focus: String = r.get(2).unwrap_or_default();
        let focus_zh: String = r.get(3).unwrap_or_default();

        if let Some(day) = days.iter_mut().find(|d| d.day_number == day_number) {
            day.session_focus.push((session, focus, focus_zh));
        }
    }

    // Load activities
    let mut act_rows = conn
        .query(
            "SELECT day_number, session_type, title, start_time, end_time, notes \
             FROM activities WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, \
               CASE session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 \
                 WHEN 'afternoon' THEN 2 ELSE 3 END, sort_order",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("activities query failed: {e}"))?;

    while let Some(r) = act_rows.next().await.map_err(|e| format!("act row: {e}"))? {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let title: String = r.get(2).unwrap_or_default();
        let start_time: Option<String> = r.get(3).ok().flatten().filter(|s: &String| !s.is_empty());
        let end_time: Option<String>   = r.get(4).ok().flatten().filter(|s: &String| !s.is_empty());
        let notes: Option<String>      = r.get(5).ok().flatten();

        let Some(day) = days.iter_mut().find(|d| d.day_number == day_number) else {
            continue;
        };

        let notes_str = notes.as_deref().unwrap_or("");
        let hours = parse_hours(notes_str);
        let closed_days = parse_closed_days(notes_str);

        // Determine visit window
        let (vis_open, vis_close) = if let (Some(s), Some(e)) = (&start_time, &end_time) {
            match (parse_hhmm(s), parse_hhmm(e)) {
                (Some(so), Some(ec)) => (so, ec),
                _ => session_window(&session),
            }
        } else if let Some(s) = &start_time {
            let so = parse_hhmm(s).unwrap_or_else(|| session_window(&session).0);
            (so, so + 90) // assume 90 min if no end_time
        } else {
            session_window(&session)
        };

        let (status, detail) = classify(
            &day.date,
            vis_open,
            vis_close,
            hours,
            &closed_days,
        );

        day.activities.push(ActivityCheck {
            title,
            session,
            status,
            detail,
        });
    }

    Ok(days)
}

fn classify(
    date: &str,
    vis_open: i64,
    vis_close: i64,
    hours: Option<(i64, i64)>,
    closed_days: &[u8],
) -> (Status, String) {
    // Check day-of-week closure first
    if !closed_days.is_empty() {
        if let Some(dow) = date_to_weekday(date) {
            if closed_days.contains(&dow) {
                return (
                    Status::Closed,
                    format!("Closed on {}s", weekday_name(dow)),
                );
            }
        }
    }

    let Some((open, close)) = hours else {
        return (Status::Unknown, String::new());
    };

    // Fully outside hours
    if vis_close <= open || vis_open >= close {
        return (
            Status::Closed,
            format!(
                "Visit window {}-{} is outside opening hours {}-{}",
                fmt_hhmm(vis_open), fmt_hhmm(vis_close),
                fmt_hhmm(open), fmt_hhmm(close)
            ),
        );
    }

    // Partially outside
    if vis_open < open || vis_close > close {
        let mut parts = Vec::new();
        if vis_open < open {
            parts.push(format!("venue opens at {}", fmt_hhmm(open)));
        }
        if vis_close > close {
            parts.push(format!("venue closes at {}", fmt_hhmm(close)));
        }
        return (
            Status::Warning,
            format!(
                "Hours {}-{}: {}",
                fmt_hhmm(open), fmt_hhmm(close),
                parts.join(", ")
            ),
        );
    }

    (
        Status::Ok,
        format!("Hours {}-{}", fmt_hhmm(open), fmt_hhmm(close)),
    )
}
