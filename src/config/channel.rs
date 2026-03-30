//! 通道配置模块，负责描述飞书、微信等消息通道的接入配置。

use std::path::PathBuf;

/// 飞书回调和 IM 回复所需的配置集合。
#[derive(Debug, Clone, Default)]
pub struct FeishuCallbackConfig {
    pub verification_token: Option<String>,
    pub encrypt_key: Option<String>,
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub open_base_url: String,
    pub require_mention: bool,
    pub audio_source_lang: Option<String>,
    pub audio_target_lang: String,
}

/// 微信 iLink Bot 通道配置。
#[derive(Debug, Clone)]
pub struct WeixinChannelConfig {
    pub enabled: bool,
    pub base_url: String,
    #[allow(dead_code)]
    pub cdn_base_url: String,
    pub state_dir: PathBuf,
    pub route_tag: Option<String>,
    pub ilink_app_id: String,
    pub bot_type: String,
    pub login_timeout_ms: u64,
    pub long_poll_timeout_ms: u64,
    pub retry_delay_ms: u64,
    pub backoff_delay_ms: u64,
    pub session_pause_minutes: u64,
}
