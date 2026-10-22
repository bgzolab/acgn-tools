//! Steam 库存（已拥有游戏）载荷解析与映射。
//!
//! 数据来自官方 `IPlayerService/GetOwnedGames/v1`：**必须带 API Key**（匿名调用会被
//! 401 拒绝；社区 XML `games?tab=all` 也已收紧到登录可见），一次请求返回全库，
//! 含 `playtime_forever`（累计分钟数）、`playtime_2weeks`、`rtime_last_played` 等。
//!
//! 隐私设置是唯一的门槛：`Game details` 非公开时 Steam 返回 `{"response": {}}`
//! （没有 `games` 字段），与「库存为空」（`game_count: 0` + `games: []`）必须区分开。
//! 所有字段宽松解析：Steam 随时可能增减字段，缺字段不能让整次导入失败。

use acgn_core::{MediaKind, MetadataLevel, Source, SourceItem, Status};
use serde::{Deserialize, Serialize};

use crate::{header_url, poster_url};

/// 单条已拥有游戏。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OwnedGame {
    pub appid: i64,
    #[serde(default)]
    pub name: Option<String>,
    /// 累计游玩时长（分钟）
    #[serde(default)]
    pub playtime_forever: Option<i64>,
    /// 最近两周游玩时长（分钟）
    #[serde(default)]
    pub playtime_2weeks: Option<i64>,
    /// 最后一次游玩的 Unix 秒
    #[serde(default)]
    pub rtime_last_played: Option<i64>,
    #[serde(default)]
    pub img_icon_url: Option<String>,
    #[serde(default)]
    pub has_community_visible_stats: Option<bool>,
    /// 家庭共享的条目（v1 不请求家庭共享，仅解析容错）
    #[serde(default)]
    pub family_shared: Option<bool>,
}

/// `GetOwnedGames` 的 `response` 对象。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OwnedGamesResponse {
    #[serde(default)]
    pub game_count: Option<i64>,
    /// 隐私受限时整个字段缺失 —— 这是与「库存为空」区分的关键
    #[serde(default)]
    pub games: Option<Vec<OwnedGame>>,
}

/// `GetOwnedGames` 的完整响应体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OwnedGamesPayload {
    #[serde(default)]
    pub response: Option<OwnedGamesResponse>,
}

impl OwnedGamesPayload {
    /// 是否是「看不见库存」（隐私设置 / 无效账号）：Steam 返回 `{"response": {}}`
    /// ——既没有 `games` 也没有 `game_count`。
    ///
    /// 只要出现了 `game_count`（哪怕 0）就说明接口正常回答了「这个人有几款游戏」，
    /// 不能把「公开但空库存」误报成隐私受限。
    pub fn privacy_blocked(&self) -> bool {
        match self.response.as_ref() {
            None => true,
            Some(response) => response.games.is_none() && response.game_count.is_none(),
        }
    }

    /// 展开成游戏列表（缺失时为空列表）。
    pub fn into_games(self) -> Vec<OwnedGame> {
        self.response
            .and_then(|response| response.games)
            .unwrap_or_default()
    }
}

/// 库存条目 → 统一 [`SourceItem`]。
///
/// 与愿望单列表档的差异：
/// * **状态**：有游玩时长的算「在玩」，没玩过的已拥有游戏算「待玩」。存储层的
///   状态收养规则会把仍是愿望单/待玩的条目升级过来，但用户手动标过的状态
///   （已通关、搁置……）不会被回退；
/// * `playtime_minutes` 带上 Steam 的累计时长（存储层按「手动优先」规则采纳）。
///   字段缺失时按 0 处理：`source_playtime_minutes` 同时是「被库存同步见过」的
///   标记（missing / 成就轮询都依赖它），不能留 NULL。
///
/// 名字缺失时 **不** 造 `Steam App <appid>` 占位符：标题属于「来源给了值才覆盖」的
/// 元数据，占位符会在下次导入时覆盖掉补全好的真实标题；空标题会被存储层保留本地值。
pub fn owned_game_to_source_item(game: &OwnedGame) -> SourceItem {
    let appid = game.appid.to_string();
    // 「玩过」的判定只看累计时长：缺字段 / 0 分钟 = 买了没玩（待玩）
    let playtime = game.playtime_forever.unwrap_or(0);
    let status = if playtime > 0 {
        Status::Playing
    } else {
        Status::Backlog
    };

    SourceItem {
        source: Source::Steam,
        external_id: appid.clone(),
        media_kind: MediaKind::Game,
        title: game.name.clone().unwrap_or_default(),
        cover_url: Some(header_url(&appid)),
        poster_url: Some(poster_url(&appid)),
        status: Some(status),
        playtime_minutes: Some(playtime),
        playtime_2weeks_minutes: game.playtime_2weeks,
        last_played_at: crate::util::from_unix(game.rtime_last_played),
        metadata: serde_json::to_value(game).ok(),
        // 库存接口只有 appid / 名字 / 时长，属于「列表档」
        metadata_level: MetadataLevel::List,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "response": {
            "game_count": 2,
            "games": [
                {
                    "appid": 1245620,
                    "name": "ELDEN RING",
                    "playtime_forever": 4621,
                    "playtime_2weeks": 0,
                    "rtime_last_played": 1760000000,
                    "img_icon_url": "abc123",
                    "content_descriptorids": [1, 5]
                },
                { "appid": 570, "playtime_forever": 0 }
            ]
        }
    }"#;

    #[test]
    fn maps_owned_games_to_source_items() {
        let payload: OwnedGamesPayload = serde_json::from_str(SAMPLE).unwrap();
        assert!(!payload.privacy_blocked());
        let games = payload.into_games();
        assert_eq!(games.len(), 2);

        let item = owned_game_to_source_item(&games[0]);
        assert_eq!(item.external_id, "1245620");
        assert_eq!(item.title, "ELDEN RING");
        assert_eq!(item.status, Some(Status::Playing), "有游玩时长 = 在玩");
        assert_eq!(item.playtime_minutes, Some(4621));
        assert_eq!(
            item.playtime_2weeks_minutes,
            Some(0),
            "最近两周时长应随账号带出"
        );
        assert_eq!(
            item.last_played_at.map(|at| at.timestamp()),
            Some(1760000000),
            "最后游玩时间应随账号带出"
        );
        assert_eq!(item.metadata_level, MetadataLevel::List);
        assert!(item.cover_url.unwrap().ends_with("1245620/header.jpg"));
        assert!(item.metadata.is_some());

        // 缺名字时不造占位符（避免覆盖本地补全好的标题）；0 分钟 = 买了没玩 → 待玩
        let item = owned_game_to_source_item(&games[1]);
        assert_eq!(item.title, "");
        assert_eq!(item.playtime_minutes, Some(0));
        assert_eq!(item.status, Some(Status::Backlog), "没玩过的算待玩");

        // 载荷缺 playtime_forever（fixture 简化形态 / Steam 改字段）时按 0 处理：
        // source_playtime_minutes 是「被库存同步见过」的标记，不能留 NULL
        let no_playtime = OwnedGame {
            appid: 400,
            name: Some("No Data".to_string()),
            ..Default::default()
        };
        let item = owned_game_to_source_item(&no_playtime);
        assert_eq!(item.playtime_minutes, Some(0));
        assert_eq!(item.status, Some(Status::Backlog));
    }

    /// 隐私受限（`{"response":{}}`）与「库存为空」必须区分：
    /// 前者要给用户明确的隐私提示，后者只是 0 条。
    #[test]
    fn distinguishes_private_response_from_empty_library() {
        let private: OwnedGamesPayload = serde_json::from_str(r#"{"response":{}}"#).unwrap();
        assert!(private.privacy_blocked());
        assert!(private.into_games().is_empty());

        // 空库存：带 game_count / games 字段，不能误报成隐私受限
        let empty: OwnedGamesPayload =
            serde_json::from_str(r#"{"response":{"game_count":0,"games":[]}}"#).unwrap();
        assert!(!empty.privacy_blocked());
        assert!(empty.into_games().is_empty());

        // 只有 game_count 的简化形态也算正常回答
        let empty: OwnedGamesPayload =
            serde_json::from_str(r#"{"response":{"game_count":0}}"#).unwrap();
        assert!(!empty.privacy_blocked());
        assert!(empty.into_games().is_empty());
    }
}
