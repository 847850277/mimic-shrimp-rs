//! 通道日志模块，负责记录飞书等外部消息通道的回调、解析和回复过程。

use serde_json::Value;
use tracing::{error, info};

use super::{preview_bytes, preview_json, preview_text};

/// 记录通道回调进入服务时的入口信息。
pub fn log_channel_callback_ingress(
    channel: &str,
    method: &str,
    uri: &str,
    user_agent: &str,
    content_type: &str,
    request_id: &str,
    body: &[u8],
) {
    info!(
        channel = %channel,
        method = %method,
        uri = %uri,
        user_agent = %user_agent,
        content_type = %content_type,
        request_id = %request_id,
        body_len = body.len(),
        raw_body_preview = %preview_bytes(body, 320),
        "received channel callback ingress"
    );
}

/// 记录读取通道回调请求体失败的情况。
pub fn log_channel_callback_body_read_error(
    channel: &str,
    method: &str,
    uri: &str,
    user_agent: &str,
    content_type: &str,
    request_id: &str,
    error_message: &str,
) {
    error!(
        channel = %channel,
        method = %method,
        uri = %uri,
        user_agent = %user_agent,
        content_type = %content_type,
        request_id = %request_id,
        error = %error_message,
        "failed to read channel callback request body"
    );
}

/// 记录通道回调 JSON 解析失败的情况。
pub fn log_channel_callback_json_error(
    channel: &str,
    method: &str,
    uri: &str,
    request_id: &str,
    raw_body_preview: &str,
    error_message: &str,
) {
    error!(
        channel = %channel,
        method = %method,
        uri = %uri,
        request_id = %request_id,
        raw_body_preview = %raw_body_preview,
        error = %error_message,
        "failed to decode channel callback json"
    );
}

/// 记录通道回调完成验签/解密后的有效负载摘要。
pub fn log_channel_callback_processed(
    channel: &str,
    encrypted: bool,
    event_type: Option<&str>,
    payload: &Value,
) {
    info!(
        channel = %channel,
        encrypted,
        event_type = ?event_type,
        payload_preview = %preview_json(payload, 240),
        "processed channel callback"
    );
}

/// 记录识别到的通道文本消息事件。
pub fn log_channel_text_message_received(
    channel: &str,
    event_id: Option<&str>,
    message_id: &str,
    chat_id: Option<&str>,
    chat_type: Option<&str>,
    session_id: &str,
    user_id: &str,
    text: &str,
) {
    info!(
        channel = %channel,
        event_id = ?event_id,
        message_id = %message_id,
        chat_id = ?chat_id,
        chat_type = ?chat_type,
        session_id = %session_id,
        user_id = %user_id,
        message_preview = %preview_text(text, 160),
        "received channel text message event"
    );
}

/// 记录识别到的通道语音消息事件。
pub fn log_channel_audio_message_received(
    channel: &str,
    event_id: Option<&str>,
    message_id: &str,
    chat_id: Option<&str>,
    chat_type: Option<&str>,
    session_id: &str,
    user_id: &str,
    file_key: &str,
    duration_ms: Option<u64>,
) {
    info!(
        channel = %channel,
        event_id = ?event_id,
        message_id = %message_id,
        chat_id = ?chat_id,
        chat_type = ?chat_type,
        session_id = %session_id,
        user_id = %user_id,
        file_key = %file_key,
        duration_ms = ?duration_ms,
        "received channel audio message event"
    );
}

/// 记录语音消息已完成转写。
pub fn log_channel_audio_transcribed(
    channel: &str,
    message_id: &str,
    session_id: &str,
    transcript: &str,
) {
    info!(
        channel = %channel,
        message_id = %message_id,
        session_id = %session_id,
        transcript_preview = %preview_text(transcript, 160),
        "transcribed channel audio message"
    );
}

/// 记录消息资源下载接口因为参数不兼容而触发的重试。
pub fn log_channel_resource_fetch_retry(
    channel: &str,
    message_id: &str,
    file_key: &str,
    attempted_type: &str,
    error_message: &str,
) {
    info!(
        channel = %channel,
        message_id = %message_id,
        file_key = %file_key,
        attempted_type = %attempted_type,
        error = %preview_text(error_message, 200),
        "retrying channel message resource fetch with fallback type"
    );
}

/// 记录被策略忽略的通道消息事件。
pub fn log_channel_message_ignored(channel: &str, reason: &str) {
    info!(channel = %channel, reason, "ignored channel message event");
}

/// 记录通道消息事件解析失败的情况。
pub fn log_channel_message_parse_error(channel: &str, error_message: &str) {
    error!(
        channel = %channel,
        error = %error_message,
        "failed to parse channel message event"
    );
}

/// 记录通道回调整体处理失败的情况。
pub fn log_channel_callback_process_error(channel: &str, status: u16, error_message: &str) {
    error!(
        channel = %channel,
        status,
        error = %error_message,
        "failed to process channel callback"
    );
}

/// 记录后台异步处理通道消息失败的情况。
pub fn log_channel_background_error(channel: &str, error_message: &str) {
    error!(
        channel = %channel,
        error = %error_message,
        "failed to process channel message event"
    );
}

/// 记录通道回复链路中的阶段性动作。
pub fn log_channel_reply_stage(channel: &str, message_id: &str, stage: &str, text: &str) {
    info!(
        channel = %channel,
        message_id = %message_id,
        stage = %stage,
        reply_preview = %preview_text(text, 160),
        "requesting channel reply"
    );
}

/// 记录通道回复接口调用成功。
pub fn log_channel_reply_success(channel: &str, message_id: &str) {
    info!(
        channel = %channel,
        message_id = %message_id,
        "channel reply api returned success"
    );
}

/// 记录通道文本消息已经成功回复给用户。
pub fn log_channel_text_replied(channel: &str, message_id: &str, session_id: &str, answer: &str) {
    info!(
        channel = %channel,
        message_id = %message_id,
        session_id = %session_id,
        answer_preview = %preview_text(answer, 160),
        "replied to channel text message"
    );
}

/// 记录通道音频回复链路中的阶段性动作。
pub fn log_channel_audio_reply_stage(
    channel: &str,
    message_id: &str,
    stage: &str,
    file_name: &str,
    format: &str,
    byte_len: usize,
    duration_ms: Option<u64>,
) {
    info!(
        channel = %channel,
        message_id = %message_id,
        stage = %stage,
        file_name = %file_name,
        format = %format,
        byte_len,
        duration_ms = ?duration_ms,
        "requesting channel audio reply"
    );
}

/// 记录通道音频消息已经成功回复给用户。
pub fn log_channel_audio_replied(
    channel: &str,
    message_id: &str,
    session_id: &str,
    file_name: &str,
    format: &str,
    duration_ms: Option<u64>,
) {
    info!(
        channel = %channel,
        message_id = %message_id,
        session_id = %session_id,
        file_name = %file_name,
        format = %format,
        duration_ms = ?duration_ms,
        "replied to channel audio message"
    );
}
