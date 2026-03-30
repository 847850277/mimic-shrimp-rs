//! 微信 iLink Bot HTTP API 客户端。

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{
    Client,
    header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use uuid::Uuid;

use crate::config::WeixinChannelConfig;

use super::types::{
    WeixinAccountRecord, WeixinBaseInfo, WeixinGetUpdatesRequest, WeixinGetUpdatesResponse,
    WeixinOutboundMessage, WeixinOutboundMessageItem, WeixinOutboundTextItem,
    WeixinQrCodeResponse, WeixinQrStatusResponse, WeixinSendMessageRequest,
};

const DEFAULT_API_TIMEOUT_MS: u64 = 15_000;

/// 微信协议客户端。
#[derive(Clone)]
pub struct WeixinApiClient {
    http: Client,
    config: WeixinChannelConfig,
    channel_version: String,
    client_version: String,
}

impl WeixinApiClient {
    /// 创建一个微信 API 客户端。
    pub fn new(config: WeixinChannelConfig) -> Self {
        Self {
            http: Client::new(),
            config,
            channel_version: env!("CARGO_PKG_VERSION").to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// 获取二维码。
    pub async fn fetch_qr_code(&self, bot_type: &str) -> Result<WeixinQrCodeResponse> {
        let endpoint = format!(
            "ilink/bot/get_bot_qrcode?bot_type={}",
            urlencoding::encode(bot_type)
        );
        let raw = self
            .api_get(self.config.base_url.as_str(), &endpoint, 5_000)
            .await?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// 轮询二维码状态。
    pub async fn poll_qr_status(
        &self,
        api_base_url: &str,
        qrcode: &str,
        timeout_ms: u64,
    ) -> Result<WeixinQrStatusResponse> {
        let endpoint = format!(
            "ilink/bot/get_qrcode_status?qrcode={}",
            urlencoding::encode(qrcode)
        );
        let raw = self.api_get(api_base_url, &endpoint, timeout_ms).await?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// 长轮询获取更新。
    pub async fn get_updates(
        &self,
        account: &WeixinAccountRecord,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> Result<WeixinGetUpdatesResponse> {
        let body = serde_json::to_string(&WeixinGetUpdatesRequest {
            get_updates_buf,
            base_info: self.base_info(),
        })?;
        let raw = match self
            .api_post(
                account.base_url.as_str(),
                "ilink/bot/getupdates",
                Some(account.bot_token.as_str()),
                body,
                timeout_ms,
            )
            .await
        {
            Ok(raw) => raw,
            Err(error) if is_timeout_error(&error) => {
                return Ok(WeixinGetUpdatesResponse {
                    ret: Some(0),
                    errcode: Some(0),
                    errmsg: None,
                    msgs: Vec::new(),
                    get_updates_buf: Some(get_updates_buf.to_string()),
                    longpolling_timeout_ms: None,
                });
            }
            Err(error) => return Err(error),
        };
        Ok(serde_json::from_str(&raw)?)
    }

    /// 发送一条文本消息。
    pub async fn send_text_message(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> Result<String> {
        let client_id = format!("mimic-shrimp-rs-{}", Uuid::new_v4());
        let payload = WeixinSendMessageRequest {
            msg: WeixinOutboundMessage {
                from_user_id: "",
                to_user_id,
                client_id: &client_id,
                message_type: 2,
                message_state: 2,
                item_list: vec![WeixinOutboundMessageItem {
                    item_type: 1,
                    text_item: WeixinOutboundTextItem { text },
                }],
                context_token,
            },
            base_info: self.base_info(),
        };
        let body = serde_json::to_string(&payload)?;
        self.api_post(
            account.base_url.as_str(),
            "ilink/bot/sendmessage",
            Some(account.bot_token.as_str()),
            body,
            DEFAULT_API_TIMEOUT_MS,
        )
        .await?;
        Ok(client_id)
    }

    fn base_info(&self) -> WeixinBaseInfo<'_> {
        WeixinBaseInfo {
            channel_version: &self.channel_version,
        }
    }

    async fn api_get(&self, base_url: &str, endpoint: &str, timeout_ms: u64) -> Result<String> {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
        let response = self
            .http
            .get(url)
            .headers(self.common_headers()?)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await?;
        Self::read_text_response(response).await
    }

    async fn api_post(
        &self,
        base_url: &str,
        endpoint: &str,
        token: Option<&str>,
        body: String,
        timeout_ms: u64,
    ) -> Result<String> {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
        let mut headers = self.auth_headers(token, body.len())?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .http
            .post(url)
            .headers(headers)
            .timeout(Duration::from_millis(timeout_ms))
            .body(body)
            .send()
            .await?;
        Self::read_text_response(response).await
    }

    fn common_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let client_version = build_client_version(self.client_version.as_str())?.to_string();
        headers.insert(
            "iLink-App-Id",
            HeaderValue::from_str(self.config.ilink_app_id.as_str())?,
        );
        headers.insert(
            "iLink-App-ClientVersion",
            HeaderValue::from_str(client_version.as_str())?,
        );
        if let Some(route_tag) = self.config.route_tag.as_deref() {
            headers.insert("SKRouteTag", HeaderValue::from_str(route_tag)?);
        }
        Ok(headers)
    }

    fn auth_headers(&self, token: Option<&str>, body_len: usize) -> Result<HeaderMap> {
        let mut headers = self.common_headers()?;
        headers.insert(
            "AuthorizationType",
            HeaderValue::from_static("ilink_bot_token"),
        );
        headers.insert(
            "X-WECHAT-UIN",
            HeaderValue::from_str(random_wechat_uin().as_str())?,
        );
        headers.insert(CONTENT_LENGTH, HeaderValue::from_str(body_len.to_string().as_str())?);
        if let Some(value) = token.filter(|value| !value.trim().is_empty()) {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(format!("Bearer {}", value.trim()).as_str())?,
            );
        }
        Ok(headers)
    }

    async fn read_text_response(response: reqwest::Response) -> Result<String> {
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("weixin api returned HTTP {}: {}", status.as_u16(), body);
        }
        Ok(body)
    }
}

fn build_client_version(version: &str) -> Result<u32> {
    let parts = version
        .split('.')
        .map(|segment| segment.parse::<u32>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("invalid client version {version}: {error}"))?;
    let major = parts.first().copied().unwrap_or_default();
    let minor = parts.get(1).copied().unwrap_or_default();
    let patch = parts.get(2).copied().unwrap_or_default();
    Ok(((major & 0xff) << 16) | ((minor & 0xff) << 8) | (patch & 0xff))
}

fn random_wechat_uin() -> String {
    let binding = Uuid::new_v4();
    let bytes = binding.as_bytes();
    let number = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    STANDARD.encode(number.to_string())
}

fn is_timeout_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .is_some_and(reqwest::Error::is_timeout)
}
