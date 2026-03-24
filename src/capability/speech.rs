//! 语音合成能力模块，负责接入 SiliconFlow 的文本转语音接口。
//! 该能力独立于现有 chat loop，直接通过 HTTP 调用音频合成 API。

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{
    Client,
    header::{CONTENT_TYPE, HeaderMap},
};
use serde_json::{Value, json};

use crate::config::SpeechSynthesisConfig;

/// 语音合成请求。
#[derive(Debug, Clone)]
pub struct SpeechSynthesisRequest {
    pub text: String,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub response_format: Option<String>,
    pub sample_rate: Option<u32>,
    pub speed: Option<f32>,
    pub gain: Option<f32>,
    pub stream: Option<bool>,
}

/// 语音合成响应。
#[derive(Debug, Clone)]
pub struct SpeechSynthesisResponse {
    pub model: String,
    pub voice: String,
    pub response_format: String,
    pub content_type: String,
    pub audio_base64: String,
    pub byte_len: usize,
    pub trace_id: Option<String>,
}

/// 语音合成能力。
#[derive(Clone)]
pub struct SpeechSynthesisCapability {
    client: Client,
    config: SpeechSynthesisConfig,
}

impl SpeechSynthesisCapability {
    /// 基于语音合成配置创建能力实例。
    pub fn new(config: SpeechSynthesisConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// 判断当前能力是否具备可用的语音合成鉴权配置。
    pub fn is_configured(&self) -> bool {
        self.config
            .api_key
            .as_deref()
            .map(str::trim)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    }

    /// 执行一次文本转语音请求。
    pub async fn execute(
        &self,
        request: SpeechSynthesisRequest,
    ) -> Result<SpeechSynthesisResponse> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "SPEECH_SYNTHESIS_API_KEY is not configured and no SiliconFlow fallback key was found"
                )
            })?;

        let resolved = ResolvedSpeechSynthesisRequest::from_request(&self.config, request)?;
        let url = format!(
            "{}/audio/speech",
            self.config.base_url.trim_end_matches('/')
        );
        let body = resolved.to_request_body();

        let response = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to call speech synthesis endpoint: {url}"))?;

        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .bytes()
            .await
            .context("failed to read speech synthesis response body")?;

        if !status.is_success() {
            let error_body = String::from_utf8_lossy(&bytes);
            bail!(
                "speech synthesis endpoint returned status {}: {}",
                status,
                error_body
            );
        }

        Ok(SpeechSynthesisResponse {
            model: resolved.model,
            voice: resolved.voice,
            response_format: resolved.response_format,
            content_type: detect_content_type(&headers),
            audio_base64: STANDARD.encode(bytes.as_ref()),
            byte_len: bytes.len(),
            trace_id: headers
                .get("x-siliconcloud-trace-id")
                .or_else(|| headers.get("x-request-id"))
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
        })
    }
}

#[derive(Debug, Clone)]
struct ResolvedSpeechSynthesisRequest {
    model: String,
    text: String,
    voice: String,
    response_format: String,
    sample_rate: Option<u32>,
    speed: f32,
    gain: f32,
    stream: bool,
}

impl ResolvedSpeechSynthesisRequest {
    /// 将外部请求与默认配置合并，得到可以直接发送的请求参数。
    fn from_request(
        config: &SpeechSynthesisConfig,
        request: SpeechSynthesisRequest,
    ) -> Result<Self> {
        let text = request.text.trim().to_string();
        if text.is_empty() {
            bail!("text must not be empty");
        }

        let model = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(config.model.as_str())
            .to_string();
        let voice = request
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(config.voice.as_deref())
            .ok_or_else(|| anyhow!("speech synthesis voice is not configured"))?
            .to_string();
        let response_format = request
            .response_format
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(config.response_format.as_str())
            .to_string();
        let speed = request.speed.unwrap_or(config.speed);
        let gain = request.gain.unwrap_or(config.gain);
        if speed <= 0.0 {
            bail!("speed must be greater than 0");
        }
        if !gain.is_finite() {
            bail!("gain must be finite");
        }

        Ok(Self {
            model,
            text,
            voice,
            response_format,
            sample_rate: request.sample_rate.or(config.sample_rate),
            speed,
            gain,
            stream: request.stream.unwrap_or(config.stream),
        })
    }

    /// 将解析后的请求参数编码为 SiliconFlow API 请求体。
    fn to_request_body(&self) -> Value {
        let mut body = json!({
            "model": self.model,
            "input": self.text,
            "voice": self.voice,
            "response_format": self.response_format,
            "speed": self.speed,
            "gain": self.gain,
            "stream": self.stream
        });

        if let Some(sample_rate) = self.sample_rate {
            body["sample_rate"] = json!(sample_rate);
        }

        body
    }
}

/// 从响应头中提取更稳妥的 content-type。
fn detect_content_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

#[cfg(test)]
mod tests {
    use super::{ResolvedSpeechSynthesisRequest, SpeechSynthesisRequest};
    use crate::config::SpeechSynthesisConfig;

    fn test_config() -> SpeechSynthesisConfig {
        SpeechSynthesisConfig {
            api_key: Some("test-key".to_string()),
            base_url: "https://api.siliconflow.cn/v1".to_string(),
            model: "MOSS-TTSD-v0.5".to_string(),
            voice: Some("alex".to_string()),
            response_format: "mp3".to_string(),
            sample_rate: Some(32_000),
            speed: 1.0,
            gain: 0.0,
            stream: false,
        }
    }

    #[test]
    fn request_resolution_falls_back_to_config_defaults() {
        let resolved = ResolvedSpeechSynthesisRequest::from_request(
            &test_config(),
            SpeechSynthesisRequest {
                text: "hello world".to_string(),
                model: None,
                voice: None,
                response_format: None,
                sample_rate: None,
                speed: None,
                gain: None,
                stream: None,
            },
        )
        .expect("resolved request");

        assert_eq!(resolved.model, "MOSS-TTSD-v0.5");
        assert_eq!(resolved.voice, "alex");
        assert_eq!(resolved.response_format, "mp3");
        assert_eq!(resolved.sample_rate, Some(32_000));
        assert!(!resolved.stream);
    }

    #[test]
    fn request_body_contains_optional_sample_rate_when_present() {
        let resolved = ResolvedSpeechSynthesisRequest::from_request(
            &test_config(),
            SpeechSynthesisRequest {
                text: "hello world".to_string(),
                model: Some("IndexTTS-2".to_string()),
                voice: Some("alex".to_string()),
                response_format: Some("wav".to_string()),
                sample_rate: Some(24_000),
                speed: Some(1.1),
                gain: Some(2.5),
                stream: Some(true),
            },
        )
        .expect("resolved request");

        let body = resolved.to_request_body();
        assert_eq!(body["model"], "IndexTTS-2");
        assert_eq!(body["response_format"], "wav");
        assert_eq!(body["sample_rate"], 24_000);
        let speed = body["speed"].as_f64().expect("speed as float");
        let gain = body["gain"].as_f64().expect("gain as float");
        assert!((speed - 1.1).abs() < 1e-6);
        assert!((gain - 2.5).abs() < 1e-6);
        assert_eq!(body["stream"], true);
    }
}
