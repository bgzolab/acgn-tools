//! 服务端配置（CLI + 环境变量）。

use acgn_steam::SteamConfig;

/// 运行期配置。
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind: String,
    pub db_path: String,
    pub web_dir: String,
    /// 允许跨域访问的来源白名单（默认只放行本机 Vite 开发端口；
    /// 同源部署与 Vite 代理都不需要 CORS）
    pub cors_origins: Vec<String>,
    pub steam: SteamConfig,
}

/// CORS 默认白名单：本机开发用，避免「任意网页都能读本地 API」。
pub fn default_cors_origins() -> Vec<String> {
    vec![
        "http://localhost:5173".to_string(),
        "http://127.0.0.1:5173".to_string(),
    ]
}

/// 从 `ACGN_CORS_ORIGINS`（逗号分隔）读取白名单。
pub fn cors_origins_from_env() -> Vec<String> {
    match std::env::var("ACGN_CORS_ORIGINS") {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .map(|origin| origin.trim().to_string())
            .filter(|origin| !origin.is_empty())
            .collect(),
        _ => default_cors_origins(),
    }
}

/// 暴露给前端的配置（不含任何密钥）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicConfig {
    pub app: &'static str,
    pub version: &'static str,
    pub schema_revision: i64,
    pub steam: PublicSteamConfig,
    pub statuses: Vec<PublicStatus>,
    pub default_per_page: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicSteamConfig {
    /// `fixture` / `live` / `live_with_key`
    pub mode: &'static str,
    pub country: String,
    pub language: String,
    pub has_api_key: bool,
    pub fixture: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicStatus {
    pub key: &'static str,
    pub label: &'static str,
}

impl AppConfig {
    pub fn public(&self) -> PublicConfig {
        PublicConfig {
            app: acgn_core::APP_NAME,
            version: acgn_core::VERSION,
            schema_revision: acgn_core::SCHEMA_REVISION,
            steam: PublicSteamConfig {
                mode: self.steam.mode(),
                country: self.steam.country.clone(),
                language: self.steam.language.clone(),
                has_api_key: self.steam.has_api_key(),
                fixture: self
                    .steam
                    .fixture
                    .as_ref()
                    .map(|path| path.display().to_string()),
            },
            statuses: acgn_core::Status::ALL
                .iter()
                .map(|status| PublicStatus {
                    key: status.as_str(),
                    label: status.label(),
                })
                .collect(),
            default_per_page: 48,
        }
    }
}
