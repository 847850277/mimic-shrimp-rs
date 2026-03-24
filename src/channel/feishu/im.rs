//! 飞书 IM 模块，负责文本消息事件解析、消息清洗以及回复 API 调用。

use anyhow::{Result, anyhow, bail};
use reqwest::{
    Client, StatusCode,
    header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    multipart::{Form, Part},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    channel::{
        ChannelKind, InboundAudioMessage, InboundMessageParseOutcome, InboundTextMessage,
        OutboundAudioReply, OutboundTextReply,
    },
    config::FeishuCallbackConfig,
    logging,
};

use super::callback::extract_event_type;

/// 飞书机器人客户端，负责获取 tenant token 并回复消息。
#[derive(Clone)]
pub struct FeishuBotClient {
    http: Client,
    config: FeishuCallbackConfig,
}

#[derive(Debug, Deserialize)]
struct TenantAccessTokenResponse {
    code: i64,
    msg: Option<String>,
    tenant_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeishuApiResponse {
    code: i64,
    msg: Option<String>,
    data: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct FeishuAudioResource {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub format: String,
}

#[derive(Debug, Deserialize)]
struct FeishuAudioMessageContent {
    file_key: String,
    #[serde(default)]
    duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FeishuFileMessageContent {
    file_key: String,
    #[serde(default)]
    file_name: Option<String>,
}

impl FeishuBotClient {
    /// 创建一个飞书机器人客户端实例。
    pub fn new(config: FeishuCallbackConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    /// 发送统一出站文本回复，内部会自动完成 access token 获取和文本格式整理。
    pub async fn send_text_reply(&self, reply: &OutboundTextReply) -> Result<()> {
        let formatted = format_reply_text_for_feishu(&reply.text);
        logging::log_channel_reply_stage(
            reply.channel.as_str(),
            &reply.reply_to_message_id,
            "tenant_access_token",
            &formatted,
        );
        let token = self.tenant_access_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reply",
            self.config.open_base_url.trim_end_matches('/'),
            reply.reply_to_message_id
        );
        logging::log_channel_reply_stage(
            reply.channel.as_str(),
            &reply.reply_to_message_id,
            "reply_api",
            &formatted,
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&build_reply_request(&formatted))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "feishu reply api returned HTTP {}: {}",
                status.as_u16(),
                body
            );
        }

        let payload: FeishuApiResponse = serde_json::from_str(&body)
            .map_err(|error| anyhow!("invalid feishu reply api response: {error}; body={body}"))?;
        if payload.code != 0 {
            bail!(
                "feishu reply api returned code {}: {}",
                payload.code,
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }
        logging::log_channel_reply_success(reply.channel.as_str(), &reply.reply_to_message_id);
        Ok(())
    }

    /// 发送统一出站音频回复，会先上传音频资源，再回复音频消息。
    pub async fn send_audio_reply(&self, reply: &OutboundAudioReply) -> Result<()> {
        let token = self.tenant_access_token().await?;
        logging::log_channel_audio_reply_stage(
            reply.channel.as_str(),
            &reply.reply_to_message_id,
            "file_upload",
            &reply.file_name,
            &reply.file_format,
            reply.bytes.len(),
            reply.duration_ms,
        );
        let file_key = self.upload_audio_file(&token, reply).await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reply",
            self.config.open_base_url.trim_end_matches('/'),
            reply.reply_to_message_id
        );
        logging::log_channel_audio_reply_stage(
            reply.channel.as_str(),
            &reply.reply_to_message_id,
            "reply_api",
            &reply.file_name,
            &reply.file_format,
            reply.bytes.len(),
            reply.duration_ms,
        );
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&build_audio_reply_request(&file_key))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "feishu audio reply api returned HTTP {}: {}",
                status.as_u16(),
                body
            );
        }

        let payload: FeishuApiResponse = serde_json::from_str(&body).map_err(|error| {
            anyhow!("invalid feishu audio reply api response: {error}; body={body}")
        })?;
        if payload.code != 0 {
            bail!(
                "feishu audio reply api returned code {}: {}",
                payload.code,
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }
        logging::log_channel_reply_success(reply.channel.as_str(), &reply.reply_to_message_id);
        Ok(())
    }

    /// 获取飞书租户访问令牌。
    async fn tenant_access_token(&self) -> Result<String> {
        let app_id = self
            .config
            .app_id
            .as_deref()
            .ok_or_else(|| anyhow!("FEISHU_APP_ID is not configured"))?;
        let app_secret = self
            .config
            .app_secret
            .as_deref()
            .ok_or_else(|| anyhow!("FEISHU_APP_SECRET is not configured"))?;
        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.config.open_base_url.trim_end_matches('/')
        );

        let response = self
            .http
            .post(url)
            .json(&json!({
                "app_id": app_id,
                "app_secret": app_secret,
            }))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "feishu tenant_access_token api returned HTTP {}: {}",
                status.as_u16(),
                body
            );
        }

        let payload: TenantAccessTokenResponse = serde_json::from_str(&body).map_err(|error| {
            anyhow!("invalid feishu tenant_access_token response: {error}; body={body}")
        })?;
        if payload.code != 0 {
            bail!(
                "feishu tenant_access_token api returned code {}: {}",
                payload.code,
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }
        payload
            .tenant_access_token
            .ok_or_else(|| anyhow!("tenant_access_token missing in feishu response"))
    }

    /// 下载飞书语音消息对应的二进制资源。
    pub async fn download_audio_resource(
        &self,
        message_id: &str,
        file_key: &str,
        resource_type: &str,
        format_hint: Option<&str>,
    ) -> Result<FeishuAudioResource> {
        let token = self.tenant_access_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/resources/{}",
            self.config.open_base_url.trim_end_matches('/'),
            message_id,
            file_key
        );
        for candidate_type in resource_type_candidates(resource_type) {
            let response = self
                .http
                .get(&url)
                .bearer_auth(&token)
                .query(&[("type", candidate_type)])
                .send()
                .await?;
            let status = response.status();
            let mime_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "audio/ogg".to_string());
            let disposition = response
                .headers()
                .get(CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string);
            let body = response.bytes().await?;

            if status.is_success() {
                let format =
                    infer_audio_format(&mime_type, disposition.as_deref(), format_hint, &body)?;
                return Ok(FeishuAudioResource {
                    bytes: body.to_vec(),
                    mime_type,
                    format,
                });
            }

            let error_body = String::from_utf8_lossy(&body).to_string();
            if should_retry_resource_type(status, &error_body, candidate_type, resource_type) {
                logging::log_channel_resource_fetch_retry(
                    "feishu",
                    message_id,
                    file_key,
                    candidate_type,
                    &error_body,
                );
                continue;
            }

            bail!(
                "feishu message resource api returned HTTP {} for type={}: {}",
                status.as_u16(),
                candidate_type,
                error_body
            );
        }

        bail!("feishu message resource api returned invalid request params for all candidate types")
    }

    async fn upload_audio_file(&self, token: &str, reply: &OutboundAudioReply) -> Result<String> {
        let url = format!(
            "{}/open-apis/im/v1/files",
            self.config.open_base_url.trim_end_matches('/')
        );
        let mut form = Form::new()
            .text(
                "file_type",
                feishu_audio_file_type(&reply.file_format).to_string(),
            )
            .text("file_name", reply.file_name.clone());
        if let Some(duration_ms) = reply.duration_ms {
            form = form.text("duration", duration_ms.to_string());
        }

        let part = Part::bytes(reply.bytes.clone())
            .file_name(reply.file_name.clone())
            .mime_str(&reply.content_type)
            .map_err(|error| anyhow!("invalid audio content type for upload: {error}"))?;
        form = form.part("file", part);

        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "feishu file upload api returned HTTP {}: {}",
                status.as_u16(),
                body
            );
        }

        let payload: FeishuApiResponse = serde_json::from_str(&body).map_err(|error| {
            anyhow!("invalid feishu file upload response: {error}; body={body}")
        })?;
        if payload.code != 0 {
            bail!(
                "feishu file upload api returned code {}: {}",
                payload.code,
                payload.msg.unwrap_or_else(|| "unknown error".to_string())
            );
        }

        payload
            .data
            .as_ref()
            .and_then(|data| data.get("file_key"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("file_key missing in feishu file upload response"))
    }
}

fn resource_type_candidates(resource_type: &str) -> Vec<&str> {
    match resource_type {
        "audio" => vec!["audio", "file"],
        "file" => vec!["file", "audio"],
        other => vec![other, "file", "audio"],
    }
}

fn should_retry_resource_type(
    status: StatusCode,
    body: &str,
    attempted_type: &str,
    original_type: &str,
) -> bool {
    status == StatusCode::BAD_REQUEST
        && body.contains("\"code\":234001")
        && attempted_type == original_type
}

/// 将飞书回调负载解析成统一的入站文本/语音消息模型。
pub fn parse_message_event(
    payload: &Value,
    config: &FeishuCallbackConfig,
) -> Result<InboundMessageParseOutcome> {
    if !matches!(extract_event_type(payload), Some("im.message.receive_v1")) {
        return Ok(InboundMessageParseOutcome::NotMessageEvent);
    }

    if payload
        .pointer("/event/sender/sender_type")
        .and_then(Value::as_str)
        == Some("app")
    {
        return Ok(InboundMessageParseOutcome::Ignored {
            reason: "ignore app-originated message event",
        });
    }

    let message_type = payload
        .pointer("/event/message/message_type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing /event/message/message_type"))?;

    let message_id = payload
        .pointer("/event/message/message_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing /event/message/message_id"))?
        .to_string();
    let chat_id = payload
        .pointer("/event/message/chat_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let chat_type = payload
        .pointer("/event/message/chat_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let event_id = payload
        .get("event_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let user_id = payload
        .pointer("/event/sender/sender_id/open_id")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .pointer("/event/sender/sender_id/user_id")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            payload
                .pointer("/event/sender/sender_id/union_id")
                .and_then(Value::as_str)
        })
        .ok_or_else(|| anyhow!("missing sender identifier in event payload"))?
        .to_string();

    if message_type == "text"
        && chat_type.as_deref() == Some("group")
        && config.require_mention
        && !payload
            .pointer("/event/message/mentions")
            .and_then(Value::as_array)
            .map(|mentions| !mentions.is_empty())
            .unwrap_or(false)
    {
        return Ok(InboundMessageParseOutcome::Ignored {
            reason: "ignore group message without bot mention",
        });
    }

    let content_raw = payload
        .pointer("/event/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing /event/message/content"))?;
    let session_seed = chat_id.clone().unwrap_or_else(|| message_id.clone());
    let session_id = format!("feishu:{session_seed}");

    match message_type {
        "text" => {
            let text = parse_text_message_content(
                content_raw,
                payload
                    .pointer("/event/message/mentions")
                    .and_then(Value::as_array),
            )?;
            Ok(InboundMessageParseOutcome::Text(InboundTextMessage {
                channel: ChannelKind::Feishu,
                event_id,
                message_id,
                chat_id,
                chat_type,
                user_id,
                session_id,
                text,
            }))
        }
        "audio" => {
            let content = parse_audio_message_content(content_raw)?;
            Ok(InboundMessageParseOutcome::Audio(InboundAudioMessage {
                channel: ChannelKind::Feishu,
                event_id,
                message_id,
                chat_id,
                chat_type,
                user_id,
                session_id,
                file_key: content.file_key,
                resource_type: "audio".to_string(),
                format_hint: None,
                duration_ms: content.duration,
            }))
        }
        "file" => {
            let content = parse_file_message_content(content_raw)?;
            let Some(format_hint) = content
                .file_name
                .as_deref()
                .and_then(infer_audio_format_from_filename)
            else {
                return Ok(InboundMessageParseOutcome::Ignored {
                    reason: "ignore non-audio file message event",
                });
            };
            Ok(InboundMessageParseOutcome::Audio(InboundAudioMessage {
                channel: ChannelKind::Feishu,
                event_id,
                message_id,
                chat_id,
                chat_type,
                user_id,
                session_id,
                file_key: content.file_key,
                resource_type: "file".to_string(),
                format_hint: Some(format_hint.to_string()),
                duration_ms: None,
            }))
        }
        _ => Ok(InboundMessageParseOutcome::Ignored {
            reason: "ignore unsupported message event",
        }),
    }
}

/// 解析飞书文本消息内容，并移除 mention key 等噪声。
fn parse_text_message_content(content_raw: &str, mentions: Option<&Vec<Value>>) -> Result<String> {
    let content: Value = serde_json::from_str(content_raw)
        .map_err(|error| anyhow!("invalid feishu text message content JSON: {error}"))?;
    let mut text = content
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing text field in feishu message content"))?
        .trim()
        .to_string();
    if let Some(mentions) = mentions {
        for mention in mentions {
            if let Some(key) = mention.get("key").and_then(Value::as_str) {
                text = text.replace(key, " ");
            }
        }
    }
    text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        bail!("feishu text message content is empty");
    }
    Ok(text)
}

/// 解析飞书语音消息内容，提取资源下载所需的 `file_key` 与时长信息。
fn parse_audio_message_content(content_raw: &str) -> Result<FeishuAudioMessageContent> {
    let content: FeishuAudioMessageContent = serde_json::from_str(content_raw)
        .map_err(|error| anyhow!("invalid feishu audio message content JSON: {error}"))?;
    if content.file_key.trim().is_empty() {
        bail!("feishu audio message content missing file_key");
    }
    Ok(content)
}

/// 解析飞书文件消息内容，提取资源下载所需的 `file_key` 与文件名。
fn parse_file_message_content(content_raw: &str) -> Result<FeishuFileMessageContent> {
    let content: FeishuFileMessageContent = serde_json::from_str(content_raw)
        .map_err(|error| anyhow!("invalid feishu file message content JSON: {error}"))?;
    if content.file_key.trim().is_empty() {
        bail!("feishu file message content missing file_key");
    }
    Ok(content)
}

/// 根据响应头推断语音资源格式，供后续转写模型使用。
fn infer_audio_format(
    mime_type: &str,
    content_disposition: Option<&str>,
    format_hint: Option<&str>,
    bytes: &[u8],
) -> Result<String> {
    if let Some(hint) = format_hint {
        let normalized = normalize_audio_format(hint);
        if !normalized.is_empty() {
            return Ok(normalized.to_string());
        }
    }

    if let Some(disposition) = content_disposition
        && let Some(filename) = extract_filename_from_content_disposition(disposition)
        && let Some(ext) = infer_audio_format_from_filename(&filename)
    {
        return Ok(ext.to_string());
    }

    if let Some(format) = infer_audio_format_from_magic(bytes) {
        return Ok(format.to_string());
    }

    let normalized = mime_type.trim().to_lowercase();
    let format = match normalized.as_str() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/aac" => "aac",
        "audio/amr" => "amr",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        _ => {
            bail!("unsupported feishu audio resource content-type: {mime_type}");
        }
    };
    Ok(format.to_string())
}

fn infer_audio_format_from_filename(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit('.').next()?;
    let normalized = normalize_audio_format(ext);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_audio_format(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "wav" => "wav",
        "mp3" => "mp3",
        "ogg" => "ogg",
        "opus" => "opus",
        "aac" => "aac",
        "amr" => "amr",
        "m4a" => "m4a",
        _ => "",
    }
}

fn infer_audio_format_from_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return Some("wav");
    }
    if bytes.starts_with(b"OggS") {
        return Some("ogg");
    }
    if bytes.starts_with(b"ID3") {
        return Some("mp3");
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return Some("mp3");
    }
    if bytes.starts_with(b"#!AMR") {
        return Some("amr");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some("m4a");
    }
    None
}

fn extract_filename_from_content_disposition(content_disposition: &str) -> Option<String> {
    content_disposition.split(';').find_map(|segment| {
        let segment = segment.trim();
        segment
            .strip_prefix("filename=")
            .or_else(|| segment.strip_prefix("filename*=UTF-8''"))
            .map(|value| value.trim_matches('"').to_string())
    })
}

/// 构造飞书文本回复请求体。
fn build_reply_request(text: &str) -> Value {
    json!({
        "content": json!({ "text": text }).to_string(),
        "msg_type": "text",
    })
}

/// 构造飞书音频回复请求体。
fn build_audio_reply_request(file_key: &str) -> Value {
    json!({
        "content": json!({ "file_key": file_key }).to_string(),
        "msg_type": "audio",
    })
}

/// 把模型回复整理成更适合飞书 IM 展示的纯文本格式。
fn format_reply_text_for_feishu(input: &str) -> String {
    let mut text = input.replace("\r\n", "\n");
    text = text.replace("**", "");
    text = text.replace("__", "");
    text = text
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let mut normalized = Vec::new();
    let mut previous_blank = false;
    for line in text.lines() {
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        normalized.push(line.trim_start().to_string());
        previous_blank = blank;
    }

    let text = normalized.join("\n").trim().to_string();
    if text.is_empty() {
        "我暂时没有可发送的回复。".to_string()
    } else {
        text
    }
}

fn feishu_audio_file_type(format: &str) -> &'static str {
    match normalize_audio_format(format) {
        "opus" | "ogg" => "opus",
        _ => "stream",
    }
}

/// 估算 Ogg/Opus 音频时长，用于飞书上传音频时补充 duration 字段。
pub fn estimate_audio_duration_ms(format: &str, bytes: &[u8]) -> Option<u64> {
    match normalize_audio_format(format) {
        "ogg" | "opus" => estimate_ogg_opus_duration_ms(bytes),
        "wav" => estimate_wav_duration_ms(bytes),
        _ => None,
    }
}

fn estimate_ogg_opus_duration_ms(bytes: &[u8]) -> Option<u64> {
    let mut cursor = 0usize;
    let mut last_granule_position = None;

    while cursor + 27 <= bytes.len() {
        if &bytes[cursor..cursor + 4] != b"OggS" {
            return None;
        }
        let segment_count = *bytes.get(cursor + 26)? as usize;
        let header_len = 27usize.checked_add(segment_count)?;
        if cursor + header_len > bytes.len() {
            return None;
        }
        let page_body_len: usize = bytes[cursor + 27..cursor + header_len]
            .iter()
            .map(|segment| *segment as usize)
            .sum();
        let page_len = header_len.checked_add(page_body_len)?;
        if cursor + page_len > bytes.len() {
            return None;
        }
        let granule_slice: [u8; 8] = bytes[cursor + 6..cursor + 14].try_into().ok()?;
        let granule_position = u64::from_le_bytes(granule_slice);
        if granule_position != u64::MAX {
            last_granule_position = Some(granule_position);
        }
        cursor += page_len;
    }

    let total_samples = last_granule_position?;
    Some(((u128::from(total_samples) * 1000) / 48_000) as u64)
}

fn estimate_wav_duration_ms(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let byte_rate = u32::from_le_bytes(bytes[28..32].try_into().ok()?);
    let data_size = u32::from_le_bytes(bytes[40..44].try_into().ok()?);
    if byte_rate == 0 {
        return None;
    }
    Some(((u128::from(data_size) * 1000) / u128::from(byte_rate)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_message_event() {
        let payload = json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event_id": "evt-1",
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_xxx"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_123",
                    "chat_id": "oc_456",
                    "chat_type": "group",
                    "message_type": "text",
                    "mentions": [
                        { "key": "@_user_1" }
                    ],
                    "content": "{\"text\":\"@bot 你是谁\"}"
                }
            }
        });

        let event = parse_message_event(
            &payload,
            &FeishuCallbackConfig {
                require_mention: true,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("parse should succeed");

        match event {
            InboundMessageParseOutcome::Text(event) => {
                assert_eq!(event.channel, ChannelKind::Feishu);
                assert_eq!(event.message_id, "om_123");
                assert_eq!(event.user_id, "ou_xxx");
                assert_eq!(event.session_id, "feishu:oc_456");
                assert_eq!(event.text, "@bot 你是谁");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn strips_feishu_mention_keys_from_text() {
        let payload = json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_xxx"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_123",
                    "chat_id": "oc_456",
                    "chat_type": "group",
                    "message_type": "text",
                    "mentions": [
                        { "key": "@_user_1" }
                    ],
                    "content": "{\"text\":\"@_user_1 我是谁\"}"
                }
            }
        });

        let event = parse_message_event(
            &payload,
            &FeishuCallbackConfig {
                require_mention: true,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("parse should succeed");

        match event {
            InboundMessageParseOutcome::Text(event) => {
                assert_eq!(event.text, "我是谁");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn ignores_group_message_without_mention_by_default() {
        let payload = json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_xxx"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_123",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}"
                }
            }
        });

        let outcome = parse_message_event(
            &payload,
            &FeishuCallbackConfig {
                require_mention: true,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("parse should succeed");
        assert_eq!(
            outcome,
            InboundMessageParseOutcome::Ignored {
                reason: "ignore group message without bot mention",
            }
        );
    }

    #[test]
    fn builds_reply_request_body() {
        assert_eq!(
            build_reply_request("hello"),
            json!({
                "content": "{\"text\":\"hello\"}",
                "msg_type": "text"
            })
        );
    }

    #[test]
    fn builds_audio_reply_request_body() {
        assert_eq!(
            build_audio_reply_request("file_v3_audio_key"),
            json!({
                "content": "{\"file_key\":\"file_v3_audio_key\"}",
                "msg_type": "audio"
            })
        );
    }

    #[test]
    fn formats_reply_text_for_feishu_as_plain_text() {
        assert_eq!(
            format_reply_text_for_feishu("**会话历史**\n\n\n- 第一条\n- 第二条\n"),
            "会话历史\n\n- 第一条\n- 第二条"
        );
    }

    #[test]
    fn estimates_ogg_opus_duration_from_last_granule_position() {
        let bytes = [
            b'O', b'g', b'g', b'S', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 1, 1, 0x41, b'O', b'g', b'g', b'S', 0, 0, 0x80, 0xBB, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0x42,
        ];
        assert_eq!(estimate_audio_duration_ms("opus", &bytes), Some(1000));
    }

    #[test]
    fn parses_audio_message_event() {
        let payload = json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event_id": "evt-audio-1",
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_audio"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_audio_123",
                    "chat_id": "oc_audio_456",
                    "chat_type": "p2p",
                    "message_type": "audio",
                    "content": "{\"file_key\":\"file_v2_audio_key\",\"duration\":2300}"
                }
            }
        });

        let event = parse_message_event(&payload, &FeishuCallbackConfig::default())
            .expect("audio event should parse");

        match event {
            InboundMessageParseOutcome::Audio(event) => {
                assert_eq!(event.channel, ChannelKind::Feishu);
                assert_eq!(event.message_id, "om_audio_123");
                assert_eq!(event.file_key, "file_v2_audio_key");
                assert_eq!(event.resource_type, "audio");
                assert_eq!(event.format_hint, None);
                assert_eq!(event.duration_ms, Some(2300));
                assert_eq!(event.session_id, "feishu:oc_audio_456");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn accepts_group_audio_message_without_mentions() {
        let payload = json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_audio"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_audio_789",
                    "chat_id": "oc_group_001",
                    "chat_type": "group",
                    "message_type": "audio",
                    "content": "{\"file_key\":\"file_v2_audio_key_2\",\"duration\":1800}"
                }
            }
        });

        let outcome = parse_message_event(
            &payload,
            &FeishuCallbackConfig {
                require_mention: true,
                ..FeishuCallbackConfig::default()
            },
        )
        .expect("audio event should parse");

        match outcome {
            InboundMessageParseOutcome::Audio(event) => {
                assert_eq!(event.session_id, "feishu:oc_group_001");
                assert_eq!(event.file_key, "file_v2_audio_key_2");
                assert_eq!(event.resource_type, "audio");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn infers_audio_format_from_headers() {
        assert_eq!(
            infer_audio_format(
                "audio/ogg",
                Some("attachment; filename=\"voice.ogg\""),
                None,
                b""
            )
            .expect("format should parse"),
            "ogg"
        );
        assert_eq!(
            infer_audio_format("audio/mpeg", None, None, b"").expect("format should parse"),
            "mp3"
        );
    }

    #[test]
    fn infers_audio_format_from_magic_bytes() {
        let wav_bytes = b"RIFF\x24\x80\x00\x00WAVEfmt ";
        assert_eq!(
            infer_audio_format("audio/octet-stream", None, None, wav_bytes)
                .expect("wav should be inferred"),
            "wav"
        );

        let ogg_bytes = b"OggS\x00\x02\x00\x00";
        assert_eq!(
            infer_audio_format("audio/octet-stream", None, None, ogg_bytes)
                .expect("ogg should be inferred"),
            "ogg"
        );
    }

    #[test]
    fn parses_audio_file_message_event() {
        let payload = json!({
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_file_audio"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_file_audio_123",
                    "chat_id": "oc_file_audio_456",
                    "chat_type": "group",
                    "message_type": "file",
                    "content": "{\"file_key\":\"file_v3_001\",\"file_name\":\"voice.wav\"}"
                }
            }
        });

        let outcome = parse_message_event(&payload, &FeishuCallbackConfig::default())
            .expect("file audio event should parse");

        match outcome {
            InboundMessageParseOutcome::Audio(event) => {
                assert_eq!(event.resource_type, "file");
                assert_eq!(event.format_hint.as_deref(), Some("wav"));
                assert_eq!(event.file_key, "file_v3_001");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn retries_audio_resource_type_after_invalid_param() {
        assert!(should_retry_resource_type(
            StatusCode::BAD_REQUEST,
            r#"{"code":234001,"msg":"Invalid request param."}"#,
            "audio",
            "audio"
        ));
        assert!(!should_retry_resource_type(
            StatusCode::BAD_REQUEST,
            r#"{"code":234001,"msg":"Invalid request param."}"#,
            "file",
            "audio"
        ));
        assert!(!should_retry_resource_type(
            StatusCode::UNAUTHORIZED,
            r#"{"code":234001,"msg":"Invalid request param."}"#,
            "audio",
            "audio"
        ));
    }
}
