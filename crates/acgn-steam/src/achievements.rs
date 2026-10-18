//! Steam 成就进度解析（`ISteamUserStats/GetPlayerAchievements`）。
//!
//! 每个游戏一次请求，响应很大（每个成就一条记录），这里只取汇总：
//! `achieved = 达成条数`、`total = 成就总数`。
//!
//! 三种回答必须区分开：
//!
//! | 响应 | 含义 | 解析结果 |
//! |---|---|---|
//! | `success: true` + `achievements` 数组 | 正常 | `Some(achieved/total)` |
//! | `success: true` 但无数组（如 Dota 2） | 该游戏没有成就集 | `Some(0/0)` |
//! | `success: false`（未拥有 / 隐私） | 取不到 | `None` |

use acgn_core::AchievementProgress;
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
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PlayerAchievementsPayload {
    #[serde(default)]
    playerstats: Option<PlayerStats>,
}

/// 解析 `GetPlayerAchievements` 响应；`None` 表示这个游戏的成就取不到。
pub fn parse_player_achievements(raw: &serde_json::Value) -> Option<AchievementProgress> {
    let payload: PlayerAchievementsPayload = serde_json::from_value(raw.clone()).ok()?;
    let stats = payload.playerstats?;

    match (stats.success, stats.achievements) {
        (_, Some(achievements)) => Some(AchievementProgress {
            achieved: achievements
                .iter()
                .filter(|item| item.achieved == 1)
                .count() as i64,
            total: achievements.len() as i64,
        }),
        // 明确成功但没有成就数组 = 游戏本身没有成就集
        (true, None) => Some(AchievementProgress {
            achieved: 0,
            total: 0,
        }),
        (false, None) => None,
    }
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
        let progress = parse_player_achievements(&raw).expect("应解析出进度");
        assert_eq!(progress.achieved, 2);
        assert_eq!(progress.total, 3);
        assert!(!progress.is_complete());
    }

    /// 全成就判定：`total > 0` 且全部达成；`0/0`（没有成就集）不算全成就。
    #[test]
    fn detects_complete_and_empty_achievement_sets() {
        let raw = serde_json::json!({
            "playerstats": { "success": true, "achievements": [
                { "achieved": 1 }, { "achieved": 1 }
            ]}
        });
        assert!(parse_player_achievements(&raw).unwrap().is_complete());

        let raw = serde_json::json!({ "playerstats": { "success": true, "gameName": "Dota 2" } });
        let progress = parse_player_achievements(&raw).expect("无成就集也是明确回答");
        assert_eq!((progress.achieved, progress.total), (0, 0));
        assert!(!progress.is_complete());

        let raw = serde_json::json!({
            "playerstats": { "error": "Profile is not public", "success": false }
        });
        assert!(
            parse_player_achievements(&raw).is_none(),
            "取不到要返回 None"
        );
    }
}
