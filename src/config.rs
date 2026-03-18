use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub server_addr: String,
    pub default_system_prompt: String,
    pub max_iterations: usize,
    pub llm: LlmConfig,
    pub feishu_callback: FeishuCallbackConfig,
    pub exec_command_tool: ExecCommandToolConfig,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub api_key: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FeishuCallbackConfig {
    pub verification_token: Option<String>,
    pub encrypt_key: Option<String>,
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub open_base_url: String,
    pub require_mention: bool,
}

#[derive(Debug, Clone)]
pub struct ExecCommandToolConfig {
    pub enabled: bool,
    pub shell: String,
    pub timeout_secs: u64,
    pub max_output_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAi,
    Glm,
    SiliconFlow,
    Gemini,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();

        let provider = LlmProvider::from_env()?;
        let model =
            std::env::var("LLM_MODEL").unwrap_or_else(|_| provider.default_model().to_string());
        let base_url = first_env(provider.base_url_envs())
            .or_else(|| provider.default_base_url().map(str::to_string));
        let api_key = first_env(provider.api_key_envs())
            .with_context(|| format!("missing one of {}", provider.api_key_envs().join(", ")))?;

        let max_iterations = match std::env::var("MAX_TOOL_ITERATIONS") {
            Ok(raw) => raw
                .parse::<usize>()
                .with_context(|| format!("invalid MAX_TOOL_ITERATIONS: {raw}"))?,
            Err(_) => 8,
        };
        if max_iterations == 0 {
            bail!("MAX_TOOL_ITERATIONS must be greater than 0");
        }

        Ok(Self {
            app_name: std::env::var("APP_NAME").unwrap_or_else(|_| "rs-tool-call".to_string()),
            server_addr: std::env::var("SERVER_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:7878".to_string()),
            default_system_prompt: std::env::var("SYSTEM_PROMPT").unwrap_or_else(|_| {
                "You are a tool-calling assistant inspired by OpenClaw. When deterministic work or external state is needed, call the available tools first, then synthesize a concise final answer.".to_string()
            }),
            max_iterations,
            llm: LlmConfig {
                provider,
                model,
                api_key,
                base_url,
            },
            feishu_callback: FeishuCallbackConfig {
                verification_token: first_env(&[
                    "FEISHU_CALLBACK_VERIFICATION_TOKEN",
                    "FEISHU_VERIFICATION_TOKEN",
                ]),
                encrypt_key: first_env(&["FEISHU_CALLBACK_ENCRYPT_KEY", "FEISHU_ENCRYPT_KEY"]),
                app_id: first_env(&["FEISHU_APP_ID", "APP_ID"]),
                app_secret: first_env(&["FEISHU_APP_SECRET", "APP_SECRET"]),
                open_base_url: std::env::var("FEISHU_OPEN_BASE_URL")
                    .unwrap_or_else(|_| "https://open.feishu.cn".to_string()),
                require_mention: parse_bool_env("FEISHU_BOT_REQUIRE_MENTION", true)
                    .with_context(|| {
                        "invalid FEISHU_BOT_REQUIRE_MENTION, expected true/false".to_string()
                    })?,
            },
            exec_command_tool: ExecCommandToolConfig {
                enabled: parse_bool_env("EXEC_COMMAND_TOOL_ENABLED", false).with_context(|| {
                    "invalid EXEC_COMMAND_TOOL_ENABLED, expected true/false".to_string()
                })?,
                shell: std::env::var("EXEC_COMMAND_TOOL_SHELL")
                    .unwrap_or_else(|_| "/bin/sh".to_string()),
                timeout_secs: parse_u64_env("EXEC_COMMAND_TOOL_TIMEOUT_SECS", 20).with_context(
                    || "invalid EXEC_COMMAND_TOOL_TIMEOUT_SECS, expected integer".to_string(),
                )?,
                max_output_chars: parse_usize_env("EXEC_COMMAND_TOOL_MAX_OUTPUT_CHARS", 4000)
                    .with_context(|| {
                        "invalid EXEC_COMMAND_TOOL_MAX_OUTPUT_CHARS, expected integer".to_string()
                    })?,
            },
        })
    }
}

impl LlmProvider {
    fn from_env() -> Result<Self> {
        let raw = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "openai".to_string());
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "glm" | "bailian" | "dashscope" => Ok(Self::Glm),
            "siliconflow" | "silicon-flow" | "silicon" => Ok(Self::SiliconFlow),
            "gemini" => Ok(Self::Gemini),
            other => Err(anyhow!("unsupported LLM_PROVIDER: {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Glm => "glm",
            Self::SiliconFlow => "siliconflow",
            Self::Gemini => "gemini",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-mini",
            Self::Glm => "glm-5",
            Self::SiliconFlow => "zai-org/GLM-4.6",
            Self::Gemini => "gemini-2.5-flash",
        }
    }

    fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::OpenAi => None,
            Self::Glm => Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            Self::SiliconFlow => Some("https://api.siliconflow.cn/v1"),
            Self::Gemini => None,
        }
    }

    fn api_key_envs(self) -> &'static [&'static str] {
        match self {
            Self::OpenAi => &["OPENAI_API_KEY", "LLM_API_KEY"],
            Self::Glm => &[
                "DASHSCOPE_API_KEY",
                "BAILIAN_API_KEY",
                "GLM_API_KEY",
                "LLM_API_KEY",
            ],
            Self::SiliconFlow => &["SILICONFLOW_API_KEY", "OPENAI_API_KEY", "LLM_API_KEY"],
            Self::Gemini => &["GOOGLE_API_KEY", "LLM_API_KEY"],
        }
    }

    fn base_url_envs(self) -> &'static [&'static str] {
        match self {
            Self::OpenAi => &["OPENAI_BASE_URL", "LLM_BASE_URL"],
            Self::Glm => &[
                "DASHSCOPE_BASE_URL",
                "BAILIAN_BASE_URL",
                "GLM_BASE_URL",
                "OPENAI_BASE_URL",
                "LLM_BASE_URL",
            ],
            Self::SiliconFlow => &["SILICONFLOW_BASE_URL", "OPENAI_BASE_URL", "LLM_BASE_URL"],
            Self::Gemini => &["LLM_BASE_URL"],
        }
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

fn parse_bool_env(key: &str, default: bool) -> Result<bool> {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(anyhow!("unsupported boolean value: {raw}")),
        },
        Err(_) => Ok(default),
    }
}

fn parse_u64_env(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse::<u64>()
            .map_err(|_| anyhow!("unsupported integer value: {raw}")),
        Err(_) => Ok(default),
    }
}

fn parse_usize_env(key: &str, default: usize) -> Result<usize> {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse::<usize>()
            .map_err(|_| anyhow!("unsupported integer value: {raw}")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::LlmProvider;

    #[test]
    fn glm_aliases_are_supported() {
        for raw in ["glm", "bailian", "dashscope"] {
            let provider = LlmProvider::parse(raw).expect("glm alias");
            assert_eq!(provider.as_str(), "glm");
        }
    }

    #[test]
    fn glm_defaults_match_bailian_openai_compatible_endpoint() {
        assert_eq!(LlmProvider::Glm.default_model(), "glm-5");
        assert_eq!(
            LlmProvider::Glm.default_base_url(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
    }

    #[test]
    fn siliconflow_aliases_are_supported() {
        for raw in ["siliconflow", "silicon-flow", "silicon"] {
            let provider = LlmProvider::parse(raw).expect("siliconflow alias");
            assert_eq!(provider.as_str(), "siliconflow");
        }
    }

    #[test]
    fn siliconflow_defaults_match_openai_compatible_endpoint() {
        assert_eq!(LlmProvider::SiliconFlow.default_model(), "zai-org/GLM-4.6");
        assert_eq!(
            LlmProvider::SiliconFlow.default_base_url(),
            Some("https://api.siliconflow.cn/v1")
        );
    }
}
