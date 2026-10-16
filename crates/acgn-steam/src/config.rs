//! 客户端配置。

use std::path::PathBuf;
use std::time::Duration;

use acgn_core::Result;

use crate::fixture::FixtureData;

/// Steam 适配层配置。
#[derive(Debug, Clone)]
pub struct SteamConfig {
    /// 可选：有 Key 时会启用官方接口作为公开接口的兜底。
    pub api_key: Option<String>,
    /// 商店国家（影响价格币种），默认 `cn`。
    pub country: String,
    /// 商店语言，默认 `schinese`。
    pub language: String,
    /// 离线 fixture 目录 / 文件；设置后完全不发网络请求。
    pub fixture: Option<PathBuf>,
    pub user_agent: String,
    pub timeout: Duration,
    /// 元数据补全并发数。
    pub enrich_concurrency: usize,
    /// 每批之间的间隔（毫秒），用于规避 appdetails 限流。
    pub enrich_delay_ms: u64,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            country: "cn".to_string(),
            language: "schinese".to_string(),
            fixture: None,
            user_agent: format!(
                "acgn-tools/{} (+https://github.com/bgzo/acgn-tools)",
                acgn_core::VERSION
            ),
            timeout: Duration::from_secs(25),
            enrich_concurrency: 2,
            enrich_delay_ms: 400,
        }
    }
}

impl SteamConfig {
    /// 从环境变量读取（服务器 / Tauri 都用这一套）。
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(key) = std::env::var("STEAM_API_KEY") {
            let key = key.trim().to_string();
            if !key.is_empty() {
                config.api_key = Some(key);
            }
        }
        if let Ok(country) = std::env::var("STEAM_COUNTRY")
            && !country.trim().is_empty()
        {
            config.country = country;
        }
        if let Ok(language) = std::env::var("STEAM_LANGUAGE")
            && !language.trim().is_empty()
        {
            config.language = language;
        }
        if let Ok(fixture) = std::env::var("ACGN_STEAM_FIXTURE") {
            let fixture = fixture.trim().to_string();
            if !fixture.is_empty() {
                config.fixture = Some(PathBuf::from(fixture));
            }
        }
        config
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    pub fn is_fixture(&self) -> bool {
        self.fixture.is_some()
    }

    /// 供前端展示的取数模式。
    pub fn mode(&self) -> &'static str {
        if self.is_fixture() {
            "fixture"
        } else if self.has_api_key() {
            "live_with_key"
        } else {
            "live"
        }
    }

    /// 载入 fixture（若配置了）。
    pub fn load_fixture(&self) -> Result<Option<FixtureData>> {
        match &self.fixture {
            Some(path) => Ok(Some(FixtureData::load(path)?)),
            None => Ok(None),
        }
    }
}
