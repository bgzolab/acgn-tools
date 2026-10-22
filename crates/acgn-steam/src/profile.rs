//! 账号资料：官方 `GetPlayerSummaries`、社区 XML 兜底、fixture。
//!
//! 账号展示需要的字段很少（昵称 / 头像 / 主页 / 区域），但要求「来源给多少存多少」：
//! 官方接口返回的 player 对象会原样放进 `AccountProfile.metadata`，以后想展示
//! 在线状态、注册时间、常用语言之类不需要再迁移数据库。
//!
//! 区域（`loccountrycode`）只是 Steam 资料的所在地，未必等于账号的实际购买区；
//! 存储层允许手动覆盖（见 `accounts.country_override`）。

use acgn_core::AccountProfile;
use serde_json::{Map, Value};

use crate::{API_BASE, COMMUNITY_BASE};

/// 官方 `GetPlayerSummaries` 地址（最多 100 个 steamid，这里一次一个）。
pub fn player_summaries_url(api_key: &str, steam_id: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("{API_BASE}/ISteamUser/GetPlayerSummaries/v2/"))
        .expect("静态 API 地址应当合法");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("key", api_key);
        pairs.append_pair("steamids", steam_id);
    }
    url.to_string()
}

/// 社区资料 XML（不需要 Key；官方接口不可用时的兜底）。
pub fn profile_xml_url(steam_id: &str) -> String {
    format!("{COMMUNITY_BASE}/profiles/{steam_id}/?xml=1")
}

/// 从 `GetPlayerSummaries` 响应里取出第一个 player 并映射成账号资料。
pub fn parse_player_summaries(value: &Value) -> Option<AccountProfile> {
    let player = value
        .get("response")?
        .get("players")?
        .as_array()?
        .first()?
        .clone();
    player_to_profile(&player)
}

/// 单个 player 对象 → 账号资料（保留完整原始载荷）。
pub fn player_to_profile(player: &Value) -> Option<AccountProfile> {
    let object = player.as_object()?;

    Some(AccountProfile {
        display_name: string_field(object, "personaname")
            .map(str::trim)
            .map(str::to_string)
            .filter(|name| !name.is_empty()),
        avatar_url: string_field(object, "avatarfull").map(str::to_string),
        profile_url: string_field(object, "profileurl").map(str::to_string),
        country_code: string_field(object, "loccountrycode").map(str::to_string),
        metadata: Some(player.clone()),
    })
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 解析社区 XML 资料（keyless 兜底）。
///
/// 只提取展示需要的字段 + 一份精简的原始标签；拿不到区域（XML 里没有），
/// 所以区域价格只能靠手动覆盖或全局配置。
pub fn parse_profile_xml(xml: &str) -> AccountProfile {
    let display_name = extract_xml_tag(xml, "steamID");
    let avatar_url = extract_xml_tag(xml, "avatarFull");
    let custom_url = extract_xml_tag(xml, "customURL");
    let steam_id64 = extract_xml_tag(xml, "steamID64");

    let profile_url = match (custom_url.as_deref(), steam_id64.as_deref()) {
        (Some(custom), _) if !custom.trim().is_empty() => {
            Some(format!("{COMMUNITY_BASE}/id/{custom}/"))
        }
        (_, Some(id)) => Some(format!("{COMMUNITY_BASE}/profiles/{id}/")),
        _ => None,
    };

    let mut metadata = Map::new();
    metadata.insert(
        "kind".to_string(),
        Value::String("community_xml".to_string()),
    );
    for tag in [
        "steamID64",
        "steamID",
        "customURL",
        "privacyState",
        "visibilityState",
        "onlineState",
        "memberSince",
        "location",
        "realname",
        "summary",
    ] {
        if let Some(value) = extract_xml_tag(xml, tag) {
            metadata.insert(tag.to_string(), Value::String(value));
        }
    }

    AccountProfile {
        display_name,
        avatar_url,
        profile_url,
        country_code: None,
        metadata: Some(Value::Object(metadata)),
    }
}

/// 从 XML 中取标签文本，并解码标准实体（昵称里 `&amp;` 很常见）。
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = &xml[start..];
    let end = rest.find(&close)?;
    let value = xml_unescape(rest[..end].trim());
    (!value.is_empty()).then_some(value)
}

/// 解码 XML 的五个标准实体 + 数字实体（够用即可，不引入 XML 依赖）。
fn xml_unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }

    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        result.push_str(&rest[..index]);
        let tail = &rest[index..];

        let Some(semicolon) = tail.find(';') else {
            result.push_str(tail);
            return result;
        };
        let entity = &tail[1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };

        match decoded {
            Some(character) => {
                result.push(character);
                rest = &tail[semicolon + 1..];
            }
            None => {
                // 不是已知实体：原样保留 `&`，继续往后找
                result.push('&');
                rest = &tail[1..];
            }
        }
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARIES: &str = r#"{
        "response": {
            "players": [
                {
                    "steamid": "76561198000000000",
                    "communityvisibilitystate": 3,
                    "profilestate": 1,
                    "personaname": "ACGN Tester",
                    "profileurl": "https://steamcommunity.com/id/acgn/",
                    "avatar": "https://avatars.example/a.jpg",
                    "avatarmedium": "https://avatars.example/a_medium.jpg",
                    "avatarfull": "https://avatars.example/a_full.jpg",
                    "personastate": 1,
                    "loccountrycode": "HK",
                    "timecreated": 1234567890
                }
            ]
        }
    }"#;

    #[test]
    fn parses_player_summaries_and_keeps_raw_payload() {
        let value: Value = serde_json::from_str(SUMMARIES).unwrap();
        let profile = parse_player_summaries(&value).expect("应有资料");
        assert_eq!(profile.display_name.as_deref(), Some("ACGN Tester"));
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://avatars.example/a_full.jpg")
        );
        assert_eq!(profile.country_code.as_deref(), Some("HK"));

        let metadata = profile.metadata.expect("应保留原始载荷");
        assert_eq!(metadata["personastate"], 1, "接口给了多少就存多少");
        assert_eq!(metadata["timecreated"], 1234567890);
    }

    #[test]
    fn empty_players_array_yields_none() {
        let value: Value = serde_json::from_str(r#"{"response":{"players":[]}}"#).unwrap();
        assert!(parse_player_summaries(&value).is_none());
    }

    #[test]
    fn parses_community_xml_fallback_with_entities() {
        let xml = r#"<?xml version="1.0"?>
        <profile>
          <steamID64>76561198000000000</steamID64>
          <steamID>Tom &amp; Jerry</steamID>
          <customURL>tom-jerry</customURL>
          <avatarFull>https://avatars.example/x_full.jpg</avatarFull>
          <privacyState>public</privacyState>
          <memberSince>2010-01-01</memberSince>
        </profile>"#;

        let profile = parse_profile_xml(xml);
        assert_eq!(profile.display_name.as_deref(), Some("Tom & Jerry"));
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://avatars.example/x_full.jpg")
        );
        assert_eq!(
            profile.profile_url.as_deref(),
            Some("https://steamcommunity.com/id/tom-jerry/")
        );
        assert!(profile.country_code.is_none(), "XML 里没有区域");
        let metadata = profile.metadata.expect("应保留提取到的标签");
        assert_eq!(metadata["privacyState"], "public");
    }

    #[test]
    fn xml_unescape_handles_numeric_entities() {
        assert_eq!(xml_unescape("A &#64; B &#x1f600;"), "A @ B 😀");
        assert_eq!(xml_unescape("100% &unknown; raw"), "100% &unknown; raw");
    }
}
