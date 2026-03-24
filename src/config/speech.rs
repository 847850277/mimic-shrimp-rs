//! 语音合成配置模块，负责声明文本转语音能力所需的运行参数。

/// 语音合成能力配置。
#[derive(Debug, Clone)]
pub struct SpeechSynthesisConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub voice: Option<String>,
    pub response_format: String,
    pub sample_rate: Option<u32>,
    pub speed: f32,
    pub gain: f32,
    pub stream: bool,
}
