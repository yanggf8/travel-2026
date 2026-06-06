#![allow(dead_code)]

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct HolidayEntry {
    pub name: String,
    pub name_en: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct MakeupWorkday {
    pub name: String,
    pub name_en: String,
    pub for_holiday: String,
}

#[derive(Debug, Clone)]
pub struct HolidayCalendar {
    pub country: String,
    pub year: i32,
    pub description: String,
    pub holidays: HashMap<String, HolidayEntry>,
    pub makeup_workdays: HashMap<String, MakeupWorkday>,
}

#[derive(Debug, Clone)]
pub struct DayDetail {
    pub date: String,
    pub day_of_week: usize,
    pub is_weekend: bool,
    pub is_holiday: bool,
    pub is_makeup_workday: bool,
    pub holiday_name: Option<String>,
    pub requires_leave: bool,
}

#[derive(Debug, Clone)]
pub struct LeaveDayResult {
    pub start_date: String,
    pub end_date: String,
    pub total_days: usize,
    pub leave_days: usize,
    pub weekend_days: usize,
    pub holiday_days: usize,
    pub breakdown: Vec<DayDetail>,
}

const DAY_NAMES_ZH: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];

pub fn year_from_date(date: &str) -> Result<i32, String> {
    Ok(parse_date(date)?.year())
}

pub fn calculate_leave_days(
    start_date: &str,
    end_date: &str,
    calendar: &HolidayCalendar,
) -> Result<LeaveDayResult, String> {
    let start = parse_date(start_date)?;
    let end = parse_date(end_date)?;
    if start > end {
        return Err("Start date must be before or equal to end date".to_string());
    }

    let mut breakdown = Vec::new();
    let mut leave_days = 0usize;
    let mut weekend_days = 0usize;
    let mut holiday_days = 0usize;
    let mut current = start;

    while current <= end {
        let date = current.format("%Y-%m-%d").to_string();
        let month_day = current.format("%m-%d").to_string();
        let day_of_week = js_day_of_week(current.weekday());
        let weekend = matches!(current.weekday(), Weekday::Sat | Weekday::Sun);
        let holiday = calendar.holidays.get(&month_day);
        let makeup = calendar.makeup_workdays.get(&month_day);
        let is_holiday = holiday.is_some();
        let is_makeup_workday = makeup.is_some();
        let requires_leave = if is_makeup_workday {
            true
        } else {
            !(is_holiday || weekend)
        };

        if requires_leave {
            leave_days += 1;
        }
        if weekend && !is_makeup_workday {
            weekend_days += 1;
        }
        if is_holiday {
            holiday_days += 1;
        }

        breakdown.push(DayDetail {
            date,
            day_of_week,
            is_weekend: weekend,
            is_holiday,
            is_makeup_workday,
            holiday_name: holiday.map(|h| h.name.clone()),
            requires_leave,
        });

        current += Duration::days(1);
    }

    Ok(LeaveDayResult {
        start_date: start_date.to_string(),
        end_date: end_date.to_string(),
        total_days: breakdown.len(),
        leave_days,
        weekend_days,
        holiday_days,
        breakdown,
    })
}

pub fn format_leave_day_table(result: &LeaveDayResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Trip: {} to {} ({} days)",
        result.start_date, result.end_date, result.total_days
    ));
    lines.push(format!("Leave days required: {}", result.leave_days));
    lines.push(format!("Weekend days: {}", result.weekend_days));
    lines.push(format!("Holiday days: {}", result.holiday_days));
    lines.push(String::new());
    lines.push("Breakdown:".to_string());
    lines.push("| Date | Day | Type | Leave |".to_string());
    lines.push("|------|-----|------|-------|".to_string());

    for day in &result.breakdown {
        let kind = if day.is_makeup_workday {
            "補班".to_string()
        } else if day.is_holiday {
            day.holiday_name
                .clone()
                .unwrap_or_else(|| "假日".to_string())
        } else if day.is_weekend {
            "週末".to_string()
        } else {
            "平日".to_string()
        };
        let leave = if day.requires_leave { "✓" } else { "" };
        lines.push(format!(
            "| {} | {} | {} | {} |",
            day.date, DAY_NAMES_ZH[day.day_of_week], kind, leave
        ));
    }

    lines.join("\n")
}

fn parse_date(date: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|err| format!("invalid date {date}: {err}"))
}

fn js_day_of_week(day: Weekday) -> usize {
    match day {
        Weekday::Sun => 0,
        Weekday::Mon => 1,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar() -> HolidayCalendar {
        let mut holidays = HashMap::new();
        holidays.insert(
            "06-22".to_string(),
            HolidayEntry {
                name: "Test Holiday".to_string(),
                name_en: String::new(),
                kind: "national".to_string(),
            },
        );
        HolidayCalendar {
            country: "taiwan".to_string(),
            year: 2026,
            description: "test".to_string(),
            holidays,
            makeup_workdays: HashMap::new(),
        }
    }

    #[test]
    fn calculates_weekends_and_holidays() {
        let result = calculate_leave_days("2026-06-20", "2026-06-24", &calendar()).unwrap();
        assert_eq!(result.total_days, 5);
        assert_eq!(result.weekend_days, 2);
        assert_eq!(result.holiday_days, 1);
        assert_eq!(result.leave_days, 2);
    }

    #[test]
    fn formats_leave_table_like_ts_cli() {
        let result = calculate_leave_days("2026-06-20", "2026-06-22", &calendar()).unwrap();
        let out = format_leave_day_table(&result);
        assert!(out.contains("Trip: 2026-06-20 to 2026-06-22 (3 days)"));
        assert!(out.contains("| 2026-06-22 | 一 | Test Holiday |  |"));
    }
}
