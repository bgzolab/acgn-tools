//! 小工具：日期 / 数字解析、URL 拼装。

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

/// 把 Steam 的各种日期写法解析成 `YYYY-MM-DD`。
///
/// 覆盖："25 Feb, 2022" / "Feb 25, 2022" / "2022 年 2 月 25 日" / "2022-02-25" / "2022/02/25"，
/// 以及兜底的正则式提取（`2022年3月` 这种没有日的也能拿到年月）。
pub fn parse_release_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let normalized = raw.replace(['年', '月', '/', '.'], "-").replace('日', "");

    // 2022-2-25 / 2022-02-25
    if let Some(date) = try_ymd(&normalized) {
        return Some(date.to_string());
    }

    // 25 Feb, 2022
    for format in [
        "%d %b, %Y",
        "%d %B, %Y",
        "%b %d, %Y",
        "%B %d, %Y",
        "%b %d %Y",
    ] {
        if let Ok(parsed) = chrono::NaiveDate::parse_from_str(raw, format) {
            return Some(parsed.to_string());
        }
    }

    None
}

fn try_ymd(value: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = value
        .split(['-', ' ', ','])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 2 {
        return None;
    }

    // 只取纯数字段，兼容 "2022 - 02 - 25" 与 "2022-2"
    let numbers: Vec<i64> = parts
        .iter()
        .filter_map(|part| part.parse::<i64>().ok())
        .collect();
    if numbers.is_empty() {
        return None;
    }
    let year = numbers[0];
    let month = numbers.get(1).copied().unwrap_or(1);
    let day = numbers.get(2).copied().unwrap_or(1);
    if !(1000..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
}

/// 把 "1,234" / "12.3万" / "1.2亿" 这类计数解析成整数。
///
/// 中文社区（Bangumi / 小黑盒这类文本）常用「万 / 亿」量级后缀，必须按乘数换算，
/// 否则 12.3 万会被解析成 12（差 4 个数量级）并原样落库展示。
pub fn parse_count(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (numeric_part, multiplier) = if let Some(rest) = trimmed.strip_suffix('万') {
        (rest, 10_000_f64)
    } else if let Some(rest) = trimmed.strip_suffix('亿') {
        (rest, 100_000_000_f64)
    } else {
        (trimmed, 1_f64)
    };

    let digits: String = numeric_part
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }

    let value = match digits.parse::<i64>() {
        Ok(value) => value as f64,
        Err(_) => digits.parse::<f64>().ok()?,
    };

    Some((value * multiplier).round() as i64)
}

/// Unix 秒 → `DateTime<Utc>`。
pub fn from_unix(seconds: Option<i64>) -> Option<DateTime<Utc>> {
    let seconds = seconds?;
    if seconds <= 0 {
        return None;
    }
    Utc.timestamp_opt(seconds, 0).single()
}

/// 宽松地取 JSON 里的数字（有的字段可能是字符串）。
pub fn value_as_i64(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => parse_count(text),
        _ => None,
    }
}

/// 从 XML 中挖出 `<steamID64>`。
pub fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = &xml[start..];
    let end = rest.find(&close)?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 数字 cents → 展示用字符串（保留 2 位）。
pub fn cents_to_display(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_dates() {
        assert_eq!(
            parse_release_date("25 Feb, 2022").as_deref(),
            Some("2022-02-25")
        );
        assert_eq!(
            parse_release_date("Feb 25, 2022").as_deref(),
            Some("2022-02-25")
        );
        assert_eq!(
            parse_release_date("2022 年 2 月 25 日").as_deref(),
            Some("2022-02-25")
        );
        assert_eq!(
            parse_release_date("2024-06-21").as_deref(),
            Some("2024-06-21")
        );
        assert_eq!(parse_release_date("即将推出"), None);
    }

    #[test]
    fn parses_counts() {
        assert_eq!(parse_count("1,234"), Some(1234));
        assert_eq!(parse_count("98,765"), Some(98765));
        assert_eq!(parse_count("暂无"), None);
        // 中文量级后缀必须换算，否则差 4 个数量级
        assert_eq!(parse_count("12.3万"), Some(123_000));
        assert_eq!(parse_count("1.2亿"), Some(120_000_000));
        assert_eq!(parse_count("3万"), Some(30_000));
    }

    #[test]
    fn extracts_steam_id_from_xml() {
        let xml = r#"<profile><steamID64>76561197960287930</steamID64></profile>"#;
        assert_eq!(
            extract_xml_tag(xml, "steamID64").as_deref(),
            Some("76561197960287930")
        );
    }
}
