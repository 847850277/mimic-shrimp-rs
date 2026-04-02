//! 微信 iLink Bot 协议相关类型。

use serde::{Deserialize, Serialize};

/// 登录开始结果。
#[derive(Debug, Clone, Serialize)]
pub struct WeixinLoginStartResult {
    pub session_key: String,
    pub qr_code_url: Option<String>,
    pub qr_code_data_url: Option<String>,
    pub message: String,
}

/// 登录等待结果。
#[derive(Debug, Clone, Serialize)]
pub struct WeixinLoginWaitResult {
    pub connected: bool,
    pub account_id: Option<String>,
    pub linked_user_id: Option<String>,
    pub message: String,
}

/// 已持久化的微信账号记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinAccountRecord {
    pub account_id: String,
    pub bot_token: String,
    pub base_url: String,
    #[serde(default)]
    pub linked_user_id: Option<String>,
    pub saved_at_ms: u64,
}

/// 对外暴露的账号状态摘要。
#[derive(Debug, Clone, Serialize)]
pub struct WeixinAccountSummary {
    pub account_id: String,
    pub linked_user_id: Option<String>,
    pub configured: bool,
    pub running: bool,
    pub saved_at_ms: u64,
    pub last_start_at_ms: Option<u64>,
    pub last_event_at_ms: Option<u64>,
    pub last_inbound_at_ms: Option<u64>,
    pub last_restart_at_ms: Option<u64>,
    pub paused_until_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// 微信监控运行时状态。
#[derive(Debug, Clone, Default)]
pub struct WeixinAccountRuntimeState {
    pub running: bool,
    pub last_start_at_ms: Option<u64>,
    pub last_event_at_ms: Option<u64>,
    pub last_inbound_at_ms: Option<u64>,
    pub last_restart_at_ms: Option<u64>,
    pub paused_until_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// get_bot_qrcode 响应。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinQrCodeResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

/// get_qrcode_status 响应。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinQrStatusResponse {
    pub status: String,
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub ilink_bot_id: Option<String>,
    #[serde(default)]
    pub baseurl: Option<String>,
    #[serde(default)]
    pub ilink_user_id: Option<String>,
    #[serde(default)]
    pub redirect_host: Option<String>,
}

/// getupdates 请求体。
#[derive(Debug, Serialize)]
pub struct WeixinGetUpdatesRequest<'a> {
    pub get_updates_buf: &'a str,
    pub base_info: WeixinBaseInfo<'a>,
}

/// getuploadurl 请求体。
#[derive(Debug, Serialize)]
pub struct WeixinGetUploadUrlRequest<'a> {
    pub filekey: &'a str,
    pub media_type: u8,
    pub to_user_id: &'a str,
    pub rawsize: usize,
    pub rawfilemd5: &'a str,
    pub filesize: usize,
    pub no_need_thumb: bool,
    pub aeskey: &'a str,
    pub base_info: WeixinBaseInfo<'a>,
}

/// getuploadurl 响应。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinGetUploadUrlResponse {
    #[serde(default)]
    pub ret: Option<i64>,
    #[serde(default)]
    pub errcode: Option<i64>,
    #[serde(default)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub upload_param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub thumb_upload_param: Option<String>,
    #[serde(default)]
    pub upload_full_url: Option<String>,
}

/// 发送文本消息请求体。
#[derive(Debug, Serialize)]
pub struct WeixinSendMessageRequest<'a> {
    pub msg: WeixinOutboundMessage<'a>,
    pub base_info: WeixinBaseInfo<'a>,
}

/// 公共 base_info。
#[derive(Debug, Clone, Serialize)]
pub struct WeixinBaseInfo<'a> {
    pub channel_version: &'a str,
}

/// getupdates 响应。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinGetUpdatesResponse {
    #[serde(default)]
    pub ret: Option<i64>,
    #[serde(default)]
    pub errcode: Option<i64>,
    #[serde(default)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub msgs: Vec<WeixinMessage>,
    #[serde(default)]
    pub get_updates_buf: Option<String>,
    #[serde(default)]
    pub longpolling_timeout_ms: Option<u64>,
}

/// 微信消息。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinMessage {
    #[serde(default)]
    pub message_id: Option<u64>,
    #[serde(default)]
    pub from_user_id: Option<String>,
    #[serde(default)]
    pub item_list: Vec<WeixinMessageItem>,
    #[serde(default)]
    pub context_token: Option<String>,
}

/// 微信消息内容项。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinMessageItem {
    #[serde(rename = "type")]
    pub item_type: u8,
    #[serde(default)]
    pub text_item: Option<WeixinTextItem>,
    #[serde(default)]
    pub voice_item: Option<WeixinVoiceItem>,
}

/// 文本项。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinTextItem {
    #[serde(default)]
    pub text: Option<String>,
}

/// 语音项。
#[derive(Debug, Clone, Deserialize)]
pub struct WeixinVoiceItem {
    #[serde(default)]
    pub text: Option<String>,
}

/// 发送文本消息时的顶层 msg。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundMessage<'a> {
    pub from_user_id: &'a str,
    pub to_user_id: &'a str,
    pub client_id: &'a str,
    pub message_type: u8,
    pub message_state: u8,
    pub item_list: Vec<WeixinOutboundMessageItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<&'a str>,
}

/// 发送消息时的 item。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundMessageItem<'a> {
    #[serde(rename = "type")]
    pub item_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_item: Option<WeixinOutboundTextItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_item: Option<WeixinOutboundVoiceItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_item: Option<WeixinOutboundVideoItem<'a>>,
}

/// 发送文本消息时的 text_item。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundTextItem<'a> {
    pub text: &'a str,
}

/// 发送语音消息时的 voice_item。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundVoiceItem<'a> {
    pub media: WeixinOutboundCdnMedia<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encode_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits_per_sample: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playtime: Option<u64>,
}

/// 发送视频消息时的 video_item。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundVideoItem<'a> {
    pub media: WeixinOutboundCdnMedia<'a>,
    pub video_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_length: Option<u64>,
}

/// CDN 媒体引用。
#[derive(Debug, Serialize)]
pub struct WeixinOutboundCdnMedia<'a> {
    pub encrypt_query_param: &'a str,
    pub aes_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_type: Option<u8>,
}

#[allow(dead_code)]
/// 上传后的语音媒体结果。
#[derive(Debug, Clone)]
pub struct WeixinUploadedVoice {
    pub encrypt_query_param: String,
    pub aes_key: String,
}

/// 上传后可发送的视频媒体结果。
#[derive(Debug, Clone)]
pub struct WeixinUploadedVideo {
    pub encrypt_query_param: String,
    pub aes_key: String,
    pub file_size_ciphertext: usize,
}

/// 登录中的二维码会话。
#[derive(Debug, Clone)]
pub struct WeixinActiveLogin {
    pub qrcode: String,
    pub qrcode_url: String,
    pub started_at_ms: u64,
    pub current_api_base_url: String,
}
