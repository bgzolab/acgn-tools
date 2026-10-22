//! Steam 游戏新闻：公告 RSS 的抓取与解析。
//!
//! Steam 每个 appid 只有一个 RSS：
//! `https://steamcommunity.com/games/{appid}/rss/?cc={country}&l={language}`。
//! 没有批量接口，所以上层按「已拥有游戏」逐个抓取、落库后由列表接口读本地库。
//!
//! RSS 的 `description` 有两种形态：纯文本，或**被 XML 转义的 HTML**。这里统一转成
//! 纯文本摘要再入库：前端只渲染文本，不需要富文本 / HTML 清洗，也没有 XSS 面。

use acgn_core::{Error, NewsEntry, Result};
use chrono::{DateTime, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::client::{ReqwestExt, SteamClient};
use crate::fixture::FixtureData;
use crate::news_feed_url;

/// 一次新闻抓取的结果。
#[derive(Debug, Clone)]
pub struct NewsFetch {
    pub entries: Vec<NewsEntry>,
    pub warnings: Vec<String>,
    /// `fixture` / `live`
    pub source_mode: &'static str,
}

impl SteamClient {
    /// 抓一个 appid 的新闻。
    ///
    /// 404 视为「这个游戏没有社区新闻页」：返回空 feed 而不是失败，避免已下架游戏
    /// 永远把同步状态标记为失败。其它 4xx/5xx 交给上层记录并继续下一个游戏。
    pub async fn fetch_news(&self, appid: &str) -> Result<NewsFetch> {
        if let Some(fixture) = self.fixture() {
            return fixture.news_for_app(appid);
        }

        let url = news_feed_url(appid, &self.config().country, &self.config().language);
        let response = self
            .get(&url, "application/rss+xml, application/xml, text/xml")
            .await?;

        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(NewsFetch {
                entries: Vec::new(),
                warnings: vec!["没有新闻页（404），按空 feed 处理".to_string()],
                source_mode: "live",
            });
        }
        if status.as_u16() == 429 {
            return Err(Error::rejected(
                "Steam 新闻 RSS 触发限流（HTTP 429），请稍后再同步",
            ));
        }
        if !status.is_success() {
            return Err(crate::client::http_error_for("Steam 新闻 RSS", status));
        }

        // 匿名访问收紧时 Steam 会把请求重定向到商店首页：拿到 HTML 不能当空 feed，
        // 否则用户会以为「这个游戏没有新闻」。
        if !response.url().path().contains("/rss") {
            return Err(Error::rejected(format!(
                "Steam 新闻 RSS 被重定向到 {}，请检查代理或稍后再试",
                response.url().path()
            )));
        }

        // 少数游戏（例如 Portal 系列）没有社区公告页：路径仍是 /rss/，但返回的是
        // Steam 的 HTML 页面而不是 RSS。按空 feed 处理，避免这些条目永远显示失败。
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if content_type.to_ascii_lowercase().contains("text/html") {
            return Ok(NewsFetch {
                entries: Vec::new(),
                warnings: vec!["没有可用的新闻 RSS（返回 HTML），按空 feed 处理".to_string()],
                source_mode: "live",
            });
        }

        let body = response.text().await.net()?;
        if !body.contains("<rss") {
            return Err(Error::upstream(
                "新闻 RSS 响应不是 RSS（可能被登录页 / 商店首页拦截）",
            ));
        }
        let entries = parse_news_feed(&body)?;
        Ok(NewsFetch {
            entries,
            warnings: Vec::new(),
            source_mode: "live",
        })
    }
}

impl FixtureData {
    /// fixture 里的新闻（`news.json` 的 `news.<appid>`）。
    pub fn news_for_app(&self, appid: &str) -> Result<NewsFetch> {
        let Some(path) = self.news_path.as_ref().filter(|path| path.exists()) else {
            return Err(Error::rejected(
                "离线 fixture 未提供 news.json：无法演示游戏新闻（重新生成 fixture 或改用真实网络）",
            ));
        };
        let entries = self.news.get(appid).cloned().unwrap_or_default();
        let warnings = if entries.is_empty() {
            vec![format!(
                "fixture ({}) 里没有 appid={appid} 的新闻，按空 feed 处理",
                path.display()
            )]
        } else {
            Vec::new()
        };
        Ok(NewsFetch {
            entries,
            warnings,
            source_mode: "fixture",
        })
    }
}

/// 解析 Steam 公告 RSS。
///
/// 会跳过缺少标题 / 链接 / 合法发布时间的条目（单条坏数据不应作废整个 feed），
/// 按 guid 去重。XML 本身不合法时返回 `Error::Upstream`。
pub fn parse_news_feed(xml: &str) -> Result<Vec<NewsEntry>> {
    let mut reader = Reader::from_str(xml);

    let mut entries = Vec::new();
    let mut current: Option<RawItem> = None;
    let mut field: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_ascii_lowercase();
                if name == "item" {
                    current = Some(RawItem::default());
                    field = None;
                    continue;
                }
                if current.is_some() && is_known_field(&name) {
                    field = Some(name);
                }
            }
            Ok(Event::End(event)) => {
                let name = event.name().as_ref().to_ascii_lowercase();
                if name == "item" {
                    if let Some(raw) = current.take()
                        && let Some(entry) = raw.into_entry()
                    {
                        entries.push(entry);
                    }
                    field = None;
                    continue;
                }
                if field.as_deref() == Some(name.as_str()) {
                    field = None;
                }
            }
            Ok(Event::Text(text)) => {
                if let (Some(item), Some(field)) = (current.as_mut(), field.as_deref()) {
                    item.push(field, &text);
                }
            }
            Ok(Event::CData(text)) => {
                if let (Some(item), Some(field)) = (current.as_mut(), field.as_deref()) {
                    item.push(field, &text);
                }
            }
            Ok(Event::GeneralRef(entity)) => {
                if let (Some(item), Some(field)) = (current.as_mut(), field.as_deref()) {
                    item.push(field, &resolve_xml_entity(&entity));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                return Err(Error::upstream(format!("新闻 RSS 解析失败: {err}")));
            }
        }
    }

    // 同一 feed 内按 guid 去重（保留第一次出现的顺序）。
    let mut seen = std::collections::HashSet::new();
    entries.retain(|entry| seen.insert(entry.guid.clone()));

    if current.is_some() {
        return Err(Error::upstream(
            "新闻 RSS 解析失败: feed 在 <item> 内提前结束（可能是 HTML 登录页）",
        ));
    }
    Ok(entries)
}

fn is_known_field(name: &str) -> bool {
    matches!(
        name,
        "title" | "description" | "link" | "guid" | "author" | "pubdate"
    )
}

#[derive(Debug, Default)]
struct RawItem {
    title: String,
    description: String,
    link: String,
    guid: String,
    author: String,
    pub_date: String,
}

impl RawItem {
    fn push(&mut self, field: &str, text: &str) {
        let slot = match field {
            "title" => &mut self.title,
            "description" => &mut self.description,
            "link" => &mut self.link,
            "guid" => &mut self.guid,
            "author" => &mut self.author,
            "pubdate" => &mut self.pub_date,
            _ => return,
        };
        slot.push_str(text);
    }

    fn into_entry(self) -> Option<NewsEntry> {
        let title = collapse_whitespace(&self.title);
        let guid = self.guid.trim().to_string();
        let link = self.link.trim().to_string();
        // guid 缺失时用链接兜底；URL 缺失时，如果 guid 本身是 HTTP 链接也能用。
        let guid = if guid.is_empty() { link.clone() } else { guid };
        let url = if link.is_empty() && guid.starts_with("http") {
            guid.clone()
        } else {
            link
        };
        if title.is_empty() || guid.is_empty() || url.is_empty() {
            return None;
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        let published_at = parse_pub_date(&self.pub_date)?;
        let author = self.author.trim();
        let summary = html_to_text(&self.description);

        Some(NewsEntry {
            guid,
            title,
            summary,
            url,
            author: (!author.is_empty()).then(|| author.to_string()),
            published_at,
        })
    }
}

/// RSS `pubDate`（RFC 2822）优先，部分 feed 可能给 RFC 3339。
fn parse_pub_date(raw: &str) -> Option<DateTime<Utc>> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc2822(value)
        .map(|date| date.with_timezone(&Utc))
        .or_else(|_| DateTime::parse_from_rfc3339(value).map(|date| date.with_timezone(&Utc)))
        .ok()
}

/// XML 实体（`&amp;` / `&#x27;`）→ 字符；未知实体保留原文。
fn resolve_xml_entity(entity: &str) -> String {
    quick_xml::escape::unescape(&format!("&{entity};"))
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| format!("&{entity};"))
}

/// HTML → 纯文本：块级标签保留换行，其余标签剥掉，再解码 HTML 实体。
fn html_to_text(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }

    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut tag: Option<String> = None;

    while let Some(ch) = chars.next() {
        if let Some(buffer) = tag.as_mut() {
            if ch == '>' {
                let name = buffer
                    .trim()
                    .trim_start_matches('/')
                    .trim_end_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if name == "br" {
                    out.push('\n');
                } else if matches!(
                    name.as_str(),
                    "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3"
                ) {
                    out.push_str("\n\n");
                }
                tag = None;
            } else {
                buffer.push(ch);
            }
            continue;
        }

        if ch == '<' {
            // 只有 `<tag` / `</tag` / `<!...` 才当标签；纯文本里的 `<`（如 "2 < 3"）保留。
            let is_tag = chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?'));
            if is_tag {
                tag = Some(String::new());
            } else {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }

    // 没有闭合 `>` 的“标签”按原文保留，避免吞掉后面的正文。
    if let Some(buffer) = tag {
        out.push('<');
        out.push_str(&buffer);
    }

    let decoded = decode_html_entities(&out);
    let text = normalize_news_text(&decoded);
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// 保留段落 / 换行，压缩行内空白；不截断正文（全文在弹窗里展示）。
fn normalize_news_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = String::with_capacity(normalized.len());
    let mut blank_run = 0;

    for raw_line in normalized.split('\n') {
        let line = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !result.is_empty() {
            // 空行 = 段落间距（两个换行）；普通换行保留一个。
            for _ in 0..if blank_run > 0 { 2 } else { 1 } {
                result.push('\n');
            }
        }
        result.push_str(&line);
        blank_run = 0;
    }

    result.trim().to_string()
}

fn decode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        if let Some(end) = after.find(';')
            && let Some(ch) = resolve_html_entity(&after[..end])
        {
            out.push(ch);
            rest = &after[end + 1..];
            continue;
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn resolve_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "hellip" => Some('…'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        _ => {
            let number = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))?;
            char::from_u32(number)
        }
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Game RSS Feed</title>
    <item>
      <title>Update &amp; News</title>
      <description>&lt;div class=&quot;bb_h2&quot;&gt;Big patch&lt;/div&gt;&lt;br&gt;Line two &amp;amp; more</description>
      <link><![CDATA[https://steamcommunity.com/games/1/announcements/detail/10]]></link>
      <pubDate>Fri, 18 Aug 2023 02:30:41 +0000</pubDate>
      <author>steam</author>
      <guid isPermaLink="true">https://steamcommunity.com/games/1/announcements/detail/10</guid>
    </item>
    <item>
      <title>Second</title>
      <description><![CDATA[Plain <b>CDATA</b> text &amp; entity]]></description>
      <link>https://steamcommunity.com/games/1/announcements/detail/11</link>
      <pubDate>Thu, 17 Aug 2023 01:00:00 +0000</pubDate>
    </item>
    <item>
      <title>Broken date</title>
      <link>https://steamcommunity.com/games/1/announcements/detail/12</link>
      <pubDate>not a date</pubDate>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_cdata_html_entities_and_skips_bad_items() {
        let entries = parse_news_feed(SAMPLE).expect("feed 应能解析");
        assert_eq!(entries.len(), 2, "坏时间的条目应被跳过");
        assert_eq!(entries[0].title, "Update & News");
        assert_eq!(
            entries[0].summary.as_deref(),
            Some("Big patch\n\nLine two & more")
        );
        assert_eq!(entries[0].author.as_deref(), Some("steam"));
        assert_eq!(
            entries[0].published_at.to_rfc3339(),
            "2023-08-18T02:30:41+00:00"
        );
        assert_eq!(
            entries[1].summary.as_deref(),
            Some("Plain CDATA text & entity")
        );
        assert_eq!(
            entries[1].url,
            "https://steamcommunity.com/games/1/announcements/detail/11"
        );
        assert_eq!(
            entries[1].guid, "https://steamcommunity.com/games/1/announcements/detail/11",
            "guid 缺失时应回退到链接"
        );
    }

    #[test]
    fn skips_non_http_links() {
        let xml = r#"<rss><channel><item>
            <title>Bad</title>
            <link>javascript:alert(1)</link>
            <guid>javascript:alert(1)</guid>
            <pubDate>Fri, 18 Aug 2023 02:30:41 +0000</pubDate>
        </item></channel></rss>"#;
        assert!(
            parse_news_feed(xml).unwrap().is_empty(),
            "非 HTTP(S) 链接不能进入列表（前端会把它当 href）"
        );
    }

    #[test]
    fn keeps_literal_less_than_in_plain_text() {
        assert_eq!(
            html_to_text("Damage scales as 2 < 3 &amp; 4 > 1").as_deref(),
            Some("Damage scales as 2 < 3 & 4 > 1")
        );
        assert_eq!(
            html_to_text("<p>One</p><p>Two</p>").as_deref(),
            Some("One\n\nTwo")
        );
    }

    #[test]
    fn does_not_truncate_long_summaries() {
        let long = "正文".repeat(3000);
        let text = html_to_text(&long).expect("长文应保留");
        assert_eq!(text, long, "新闻正文不能在解析阶段被截断");
    }

    #[test]
    fn rejects_invalid_xml() {
        let err = parse_news_feed("<rss><channel><item>").expect_err("坏 XML 应报错");
        assert!(matches!(err, Error::Upstream(_)));
    }

    #[test]
    fn deduplicates_same_guid() {
        let xml = r#"<rss><channel>
            <item><title>A</title><link>https://x/1</link><guid>https://x/1</guid><pubDate>Fri, 18 Aug 2023 02:30:41 +0000</pubDate></item>
            <item><title>B</title><link>https://x/1</link><guid>https://x/1</guid><pubDate>Fri, 18 Aug 2023 02:30:41 +0000</pubDate></item>
        </channel></rss>"#;
        assert_eq!(parse_news_feed(xml).unwrap().len(), 1);
    }
}
