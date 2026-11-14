// `travel set-weather-provider <provider_id> --name-zh <text> --name-en <text> --url <https://..> --license <text>`
// `travel weather-providers`
// — upsert / list rows of `weather_providers`, the GLOBAL reference table naming the
// source of stored forecasts and the licence that must be credited (Open-Meteo: CC BY 4.0).
// The dashboard renders 「天氣資料：<name>（<license>）」 under a day's weather from these
// rows. NO --plan-id, NO audit triad (same family as set-clothing-tip). Plain text only.

use travel_db::repo::weather_providers::{self, WeatherProvider};

const VALUE_FLAGS: [&str; 4] = ["--name-zh", "--name-en", "--url", "--license"];

fn usage() -> &'static str {
    "Usage:\n  travel set-weather-provider <provider_id> --name-zh <text> --name-en <text> \
     --url <http(s)://...> --license <text>\n  travel weather-providers\n  \
     (GLOBAL reference data — no --plan-id; insert-or-replace by provider_id, the value stored in \
     days.weather_source_id / day_weather_sessions.source)"
}

fn parse_set(args: &[String]) -> Result<WeatherProvider, String> {
    if args.iter().any(|a| a == "--plan-id") {
        return Err("set-weather-provider is global reference data and takes no --plan-id".to_string());
    }
    crate::plan_resolver::reject_unknown_flags(args, &VALUE_FLAGS, &[])?;
    let (mut zh, mut en, mut url, mut lic) = (None, None, None, None);
    let mut positional: Vec<&String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if VALUE_FLAGS.contains(&a) {
            let v = args.get(i + 1).cloned().ok_or_else(|| format!("{a} needs a value"))?;
            match a {
                "--name-zh" => zh = Some(v),
                "--name-en" => en = Some(v),
                "--url" => url = Some(v),
                _ => lic = Some(v),
            }
            i += 2;
        } else {
            positional.push(&args[i]);
            i += 1;
        }
    }
    let provider_id = match positional.as_slice() {
        [id] if !id.trim().is_empty() => id.trim().to_string(),
        _ => return Err(format!("expected exactly one <provider_id>\n{}", usage())),
    };
    let need = |v: Option<String>, flag: &str| -> Result<String, String> {
        match v.map(|s| s.trim().to_string()) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(format!("{flag} is required (non-empty)")),
        }
    };
    let attribution_url = need(url, "--url")?;
    if !(attribution_url.starts_with("https://") || attribution_url.starts_with("http://")) {
        return Err(format!("--url must start with http:// or https:// (got '{attribution_url}')"));
    }
    Ok(WeatherProvider {
        provider_id,
        name_zh: need(zh, "--name-zh")?,
        name_en: need(en, "--name-en")?,
        attribution_url,
        license: need(lic, "--license")?,
    })
}

async fn print_all(conn: &libsql::Connection) -> Result<(), String> {
    let all = weather_providers::list(conn).await?;
    println!("weather_providers ({}):", all.len());
    println!("  provider_id\tname_zh\tname_en\tlicense\turl");
    for p in all {
        println!("  {}\t{}\t{}\t{}\t{}", p.provider_id, p.name_zh, p.name_en, p.license, p.attribution_url);
    }
    Ok(())
}

pub async fn run_set(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let p = parse_set(args)?;
    let conn = crate::db::connect_write().await?;
    let affected = weather_providers::upsert(&conn, &p).await?;
    if affected != 1 {
        return Err(format!("weather_providers upsert affected {affected} rows (expected 1) for {}", p.provider_id));
    }
    println!("✅ weather_providers upserted: {} ({} / {}, {})", p.provider_id, p.name_zh, p.name_en, p.license);
    println!("  url: {}", p.attribution_url);
    println!();
    print_all(&conn).await
}

pub async fn run_list(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let conn = crate::db::connect_read().await?;
    print_all(&conn).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn parses_full_args() {
        let p = parse_set(&a(&["x", "--name-zh", "甲", "--name-en", "X", "--url", "https://x.test/", "--license", "CC"])).unwrap();
        assert_eq!(p.provider_id, "x");
        assert_eq!(p.license, "CC");
    }
    #[test]
    fn rejects_missing_and_bad_url() {
        assert!(parse_set(&a(&["x", "--name-zh", "甲", "--name-en", "X", "--license", "CC"])).is_err());
        assert!(parse_set(&a(&["x", "--name-zh", "甲", "--name-en", "X", "--url", "ftp://x", "--license", "CC"])).is_err());
        assert!(parse_set(&a(&["--name-zh", "甲", "--name-en", "X", "--url", "https://x", "--license", "CC"])).is_err());
        assert!(parse_set(&a(&["x", "--plan-id", "p"])).is_err());
    }
}
