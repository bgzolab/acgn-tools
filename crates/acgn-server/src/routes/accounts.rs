//! 数据源账号接口：列表、添加（解析 ID 并抓资料）、修改价格区域、删除。
//!
//! 「添加账号」是幂等的：同一个 steamID64 重复解析只会刷新资料，不会产生第二条记录；
//! 同步接口在收到 `steam_id` 时也会走同一条建号逻辑（见 `routes::imports`）。

use acgn_core::{Account, AccountPatch, AccountSummary, Error, Source};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::extract::{ValidJson, ValidPath};
use crate::state::AppState;

/// GET /api/accounts —— 全部账号 + 库存 / 愿望单条目数。
pub async fn list_accounts(State(state): State<AppState>) -> ApiResult<Json<Vec<AccountSummary>>> {
    Ok(Json(state.db.list_accounts().await?))
}

/// POST /api/accounts/resolve —— 输入 ID / 个性域名 / 主页链接，建号并抓资料。
#[derive(Debug, Deserialize)]
pub struct ResolveAccountRequest {
    /// 数据源；目前只支持 steam（缺省即 steam）
    #[serde(default)]
    pub source: Option<Source>,
    /// 17 位 steamID64、个性域名或个人主页链接
    pub input: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveAccountResponse {
    pub account: Account,
    /// 本次是否新建（false = 已存在，资料被刷新）
    pub created: bool,
    /// `numeric` / `vanity` / `fixture`
    pub resolved_via: &'static str,
    pub warnings: Vec<String>,
}

pub async fn resolve_account(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<ResolveAccountRequest>,
) -> ApiResult<Json<ResolveAccountResponse>> {
    let source = request.source.unwrap_or(Source::Steam);
    if source != Source::Steam {
        return Err(ApiError(Error::invalid(format!(
            "暂不支持 {} 账号，目前只有 Steam",
            source.label()
        ))));
    }

    let input = request.input.trim();
    if input.is_empty() {
        return Err(ApiError(Error::invalid(
            "input 不能为空：支持 17 位 steamID64、个性域名或个人主页链接",
        )));
    }

    let (steam_id, resolved_via, mut warnings) = state.steam.resolve_id(input).await?;
    let profile = match state.steam.fetch_profile(&steam_id).await {
        Ok(profile) => Some(profile),
        Err(err) => {
            // 资料抓取失败不阻塞建号：同步库存 / 愿望单仍然可用
            warnings.push(format!("账号资料获取失败：{err}"));
            None
        }
    };

    let (account, created) = state
        .db
        .upsert_account(Source::Steam, &steam_id, profile.as_ref())
        .await?;

    Ok(Json(ResolveAccountResponse {
        account,
        created,
        resolved_via,
        warnings,
    }))
}

/// PATCH /api/accounts/{id} —— 修改价格区域（`country_code: ""` 表示跟随资料）。
pub async fn patch_account(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
    ValidJson(patch): ValidJson<AccountPatch>,
) -> ApiResult<Json<Account>> {
    let Some(raw) = patch.country_code else {
        // 没有字段 = 不改动；直接返回当前账号，避免前端误以为失败
        let account = state
            .db
            .get_account(id)
            .await?
            .ok_or_else(|| ApiError(Error::not_found(format!("账号 #{id}"))))?;
        return Ok(Json(account));
    };

    let trimmed = raw.trim();
    let country = if trimmed.is_empty() {
        None
    } else if trimmed.len() == 2 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(trimmed.to_ascii_lowercase())
    } else {
        return Err(ApiError(Error::invalid(
            "区域代码必须是两位字母（例如 cn / us / ar）；留空表示跟随 Steam 资料",
        )));
    };

    let account = state
        .db
        .set_account_country_override(id, country.as_deref())
        .await?;
    Ok(Json(account))
}

/// DELETE /api/accounts/{id} —— 删除账号（关联级联删除，游戏条目保留）。
pub async fn delete_account(
    State(state): State<AppState>,
    ValidPath(id): ValidPath<i64>,
) -> ApiResult<StatusCode> {
    if state.db.delete_account(id).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    Err(ApiError(Error::not_found(format!("账号 #{id}"))))
}
