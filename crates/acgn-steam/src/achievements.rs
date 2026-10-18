//! Steam 成就进度解析（`ISteamUserStats/GetPlayerAchievements`）。
//!
//! 每个游戏一次请求，响应很大（每个成就一条记录），这里只取汇总：
//! `achieved = 达成条数`、`total = 成就总数`。
//!
//! **状态码不是错误信号，载荷才是**（实测 2026-10）：
//!
//! | HTTP | 载荷 | 含义 | 解析结果 |
//! |---|---|---|---|
//! | 200 | `success:true` + `achievements` | 正常 | `Some(achieved/total)` |
//! | 200 | `success:true` 但无数组（如 Dota 2） | 该游戏有统计但没成就集 | `Some(0/0)` |
//! | 400 | `error: "Requested app has no stats"` | 该游戏没有统计 | `Some(0/0)` |
//! | 403 | `error: "Profile is not public"` | 未拥有 / 隐私，取不到 | `None` |
//!
//! 调用方不能因为 4xx 直接判死：400/403 都会带可解析的 JSON 答案。

use acgn_core::{AchievementProgress, Error, Result};
use serde::Deserialize;

/// 单条成就记录（只需要达成标记，其余字段宽松忽略）。
#[derive(Debug, Clone, Default, Deserialize)]
struct PlayerAchievement {
    #[serde(default)]
    achieved: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PlayerStats {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    achievements: Option<Vec<PlayerAchievement>>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PlayerAchievementsPayload {
    #[serde(default)]
    playerstats: Option<PlayerStats>,
}

/// 解析 `GetPlayerAchievements` 响应。
///
/// * `Ok(Some(progress))`：正常回答（含「该游戏没有统计 / 成就集」的 `0/0`）；
/// * `Ok(None)`：已知的「取不到」（未拥有 / 隐私）——只刷新轮询时钟；
/// * `Err(upstream)`：**未知载荷**（API 变更、代理错误页、新错误文案……）——
///   必须走失败路径，不能让整轮轮询「检查过了但什么也没学到」还推走时钟。
pub fn parse_player_achievements(raw: &serde_json::Value) -> Result<Option<AchievementProgress>> {
    let payload: PlayerAchievementsPayload = serde_json::from_value(raw.clone())
        .map_err(|err| Error::upstream(format!("成就响应结构无法识别: {err}")))?;
    let Some(stats) = payload.playerstats else {
        return Err(Error::upstream("成就响应缺少 playerstats"));
    };

    if let Some(achievements) = stats.achievements {
        return Ok(Some(AchievementProgress {
            achieved: achievements
                .iter()
                .filter(|item| item.achieved == 1)
                .count() as i64,
            total: achievements.len() as i64,
        }));
    }

    // 「明确成功但没有成就数组」= 游戏本身没有成就集
    if stats.success {
        return Ok(Some(AchievementProgress {
            achieved: 0,
            total: 0,
        }));
    }

    let Some(error) = stats.error.as_deref() else {
        return Err(Error::upstream("成就接口返回 success:false 且没有错误信息"));
    };
    let lowered = error.to_ascii_lowercase();

    // 400 的「该游戏没有统计」也是明确回答
    if lowered.contains("no stats") {
        return Ok(Some(AchievementProgress {
            achieved: 0,
            total: 0,
        }));
    }
    // 未拥有 / 隐私：已知的「取不到」
    if lowered.contains("not public") {
        return Ok(None);
    }

    Err(Error::upstream(format!("成就接口返回未知错误: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_achieved_and_total() {
        let raw = serde_json::json!({
            "playerstats": {
                "steamID": "7656119…",
                "gameName": "Slay the Spire",
                "success": true,
                "achievements": [
                    { "apiname": "A", "achieved": 1, "unlocktime": 1 },
                    { "apiname": "B", "achieved": 0, "unlocktime": 0 },
                    { "apiname": "C", "achieved": 1, "unlocktime": 2 }
                ]
            }
        });
        let progress = parse_player_achievements(&raw)
            .expect("解析不应失败")
            .expect("应解析出进度");
        assert_eq!(progress.achieved, 2);
        assert_eq!(progress.total, 3);
        assert!(!progress.is_complete());
    }

    /// 各种「没有成就」与「取不到」的形态必须区分开。
    #[test]
    fn distinguishes_empty_sets_from_unavailable_stats() {
        // 全成就
        let raw = serde_json::json!({
            "playerstats": { "success": true, "achievements": [
                { "achieved": 1 }, { "achieved": 1 }
            ]}
        });
        assert!(
            parse_player_achievements(&raw)
                .unwrap()
                .unwrap()
                .is_complete()
        );

        // 有统计但没有成就集（Dota 2 这种）
        let raw = serde_json::json!({ "playerstats": { "success": true, "gameName": "Dota 2" } });
        let progress = parse_player_achievements(&raw)
            .expect("无成就集也是明确回答")
            .expect("0/0 是 Some");
        assert_eq!((progress.achieved, progress.total), (0, 0));

        // HTTP 400：该游戏没有统计（实际是有效回答）
        let raw = serde_json::json!({
            "playerstats": { "error": "Requested app has no stats", "success": false }
        });
        let progress = parse_player_achievements(&raw)
            .expect("「没有统计」应视为 0/0")
            .expect("0/0 是 Some");
        assert_eq!((progress.achieved, progress.total), (0, 0));

        // HTTP 403：未拥有 / 隐私 —— 已知的「取不到」
        let raw = serde_json::json!({
            "playerstats": { "error": "Profile is not public", "success": false }
        });
        assert!(
            parse_player_achievements(&raw).unwrap().is_none(),
            "取不到要返回 None"
        );
    }

    /// 未知错误载荷（API 变更 / 代理错误页 / 新错误文案）必须走失败路径，
    /// 不能被吞成「该账号没有」——那会让整轮轮询「检查过了但没学到东西」。
    #[test]
    fn unknown_error_payloads_are_reported_as_upstream_errors() {
        let raw = serde_json::json!({
            "playerstats": { "error": "Please verify your key= parameter", "success": false }
        });
        assert!(parse_player_achievements(&raw).is_err());

        // 连 playerstats 都没有的载荷同样不能当成「取不到」
        let raw = serde_json::json!({ "error": "upstream is down" });
        assert!(parse_player_achievements(&raw).is_err());
    }
}
