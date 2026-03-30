//! Web 类型模块，集中定义 HTTP 请求体、响应体以及默认值辅助函数。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `/chat` 接口的请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct ChatRequest {
    #[serde(default = "default_session_id")]
    pub(crate) session_id: String,
    #[serde(default = "default_user_id")]
    pub(crate) user_id: String,
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) system_prompt: Option<String>,
    #[serde(default)]
    pub(crate) max_iterations: Option<usize>,
    #[serde(default = "default_persist")]
    pub(crate) persist: bool,
}

/// `/tools/invoke` 接口的请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct ToolInvokeRequest {
    pub(crate) tool: String,
    #[serde(default = "default_session_id")]
    pub(crate) session_id: String,
    #[serde(default = "default_user_id")]
    pub(crate) user_id: String,
    #[serde(default)]
    pub(crate) action: Option<String>,
    #[serde(default = "default_args")]
    pub(crate) args: Value,
}

/// `/extract/form` 接口的请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct FormExtractRequest {
    #[serde(default)]
    pub(crate) form_id: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) schema: Option<Value>,
    #[serde(default)]
    pub(crate) instructions: Option<String>,
}

/// 媒体翻译接口中的音频输入结构。
#[derive(Debug, Deserialize)]
pub(crate) struct MediaTranslateAudioInputRequest {
    pub(crate) data: String,
    pub(crate) format: String,
}

/// 媒体翻译接口中的音频输出结构。
#[derive(Debug, Deserialize)]
pub(crate) struct MediaTranslateAudioOutputRequest {
    pub(crate) format: String,
    pub(crate) voice: String,
}

/// `/translate/media` 接口的请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct MediaTranslateRequest {
    #[serde(default)]
    pub(crate) source_lang: Option<String>,
    pub(crate) target_lang: String,
    #[serde(default)]
    pub(crate) audio: Option<MediaTranslateAudioInputRequest>,
    #[serde(default)]
    pub(crate) video_url: Option<String>,
    #[serde(default)]
    pub(crate) output_audio: Option<MediaTranslateAudioOutputRequest>,
    #[serde(default = "default_include_usage")]
    pub(crate) include_usage: bool,
}

/// `/speech/synthesize` 接口的请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct SpeechSynthesisRequest {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) voice: Option<String>,
    #[serde(default)]
    pub(crate) response_format: Option<String>,
    #[serde(default)]
    pub(crate) sample_rate: Option<u32>,
    #[serde(default)]
    pub(crate) speed: Option<f32>,
    #[serde(default)]
    pub(crate) gain: Option<f32>,
    #[serde(default)]
    pub(crate) stream: Option<bool>,
}

/// `/weixin/login/start` 请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct WeixinLoginStartRequest {
    #[serde(default)]
    pub(crate) force: bool,
    #[serde(default)]
    pub(crate) account_id: Option<String>,
}

/// `/weixin/login/wait` 请求体。
#[derive(Debug, Deserialize)]
pub(crate) struct WeixinLoginWaitRequest {
    pub(crate) session_key: String,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
}

/// 微信登录开始响应。
#[derive(Debug, Serialize)]
pub(crate) struct WeixinLoginStartResponse {
    pub(crate) ok: bool,
    pub(crate) session_key: String,
    pub(crate) qr_code_url: Option<String>,
    pub(crate) message: String,
}

/// 微信登录等待响应。
#[derive(Debug, Serialize)]
pub(crate) struct WeixinLoginWaitResponse {
    pub(crate) ok: bool,
    pub(crate) connected: bool,
    pub(crate) account_id: Option<String>,
    pub(crate) linked_user_id: Option<String>,
    pub(crate) message: String,
}

/// 错误响应外层结构。
#[derive(Debug, Serialize)]
pub(crate) struct ErrorBody {
    pub(crate) ok: bool,
    pub(crate) error: ErrorPayload,
}

/// 错误响应中的具体错误信息。
#[derive(Debug, Serialize)]
pub(crate) struct ErrorPayload {
    pub(crate) r#type: &'static str,
    pub(crate) message: String,
}

/// 直接工具调用接口的成功响应体。
#[derive(Debug, Serialize)]
pub(crate) struct ToolInvokeResponse {
    pub(crate) ok: bool,
    pub(crate) result: Value,
}

/// `/extract/form` 接口中的单字段校验问题。
#[derive(Debug, Serialize)]
pub(crate) struct FormInvalidFieldResponse {
    pub(crate) field: String,
    pub(crate) message: String,
}

/// `/extract/form` 接口的成功响应体。
#[derive(Debug, Serialize)]
pub(crate) struct FormExtractResponse {
    pub(crate) ok: bool,
    pub(crate) form_id: Option<String>,
    pub(crate) form_title: Option<String>,
    pub(crate) schema_source: &'static str,
    pub(crate) raw_text: String,
    pub(crate) data: Value,
    pub(crate) missing_fields: Vec<String>,
    pub(crate) invalid_fields: Vec<FormInvalidFieldResponse>,
    pub(crate) warnings: Vec<String>,
}

/// `/translate/media` 接口的成功响应体。
#[derive(Debug, Serialize)]
pub(crate) struct MediaTranslateResponse {
    pub(crate) ok: bool,
    pub(crate) model: String,
    pub(crate) request_id: Option<String>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) source_lang: Option<String>,
    pub(crate) target_lang: String,
    pub(crate) translated_text: String,
    pub(crate) audio_base64: Option<String>,
    pub(crate) audio_id: Option<String>,
    pub(crate) usage: Option<Value>,
}

/// `/speech/synthesize` 接口的成功响应体。
#[derive(Debug, Serialize)]
pub(crate) struct SpeechSynthesisResponse {
    pub(crate) ok: bool,
    pub(crate) model: String,
    pub(crate) voice: String,
    pub(crate) response_format: String,
    pub(crate) content_type: String,
    pub(crate) audio_base64: String,
    pub(crate) byte_len: usize,
    pub(crate) trace_id: Option<String>,
}

/// 健康检查接口的响应体。
#[derive(Debug, Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) status: &'static str,
    pub(crate) app_name: String,
    pub(crate) provider: String,
    pub(crate) model: String,
}

/// 生成默认的空参数对象。
fn default_args() -> Value {
    Value::Object(Map::new())
}

/// 生成默认会话 ID。
fn default_session_id() -> String {
    "main".to_string()
}

/// 生成默认用户 ID。
fn default_user_id() -> String {
    "anonymous".to_string()
}

/// 生成默认的会话持久化开关值。
fn default_persist() -> bool {
    true
}

/// 生成默认的 usage 输出开关值。
fn default_include_usage() -> bool {
    true
}
