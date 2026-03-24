//! 应用配置模块，负责从环境变量装配整个服务运行所需的总配置。

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::{
    EnglishLearningConfig, ExecCommandToolConfig, FeishuCallbackConfig, FormConfig, LlmConfig,
    LlmProvider, MediaTranslateConfig,
    env::{first_env, parse_bool_env, parse_csv_env, parse_u64_env, parse_usize_env},
};

/// 应用总配置，聚合了服务自身、模型、通道和工具相关配置。
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub server_addr: String,
    pub default_system_prompt: String,
    pub max_iterations: usize,
    pub llm: LlmConfig,
    pub forms: FormConfig,
    pub media_translate: MediaTranslateConfig,
    pub english_learning: EnglishLearningConfig,
    pub feishu_callback: FeishuCallbackConfig,
    pub exec_command_tool: ExecCommandToolConfig,
}

impl AppConfig {
    /// 从环境变量加载应用全部配置。
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
            Err(_) => 12,
        };
        if max_iterations == 0 {
            bail!("MAX_TOOL_ITERATIONS must be greater than 0");
        }

        let english_learning_schedule_hour = match std::env::var("ENGLISH_LEARNING_SCHEDULE_HOUR") {
            Ok(raw) => raw
                .parse::<u32>()
                .with_context(|| format!("invalid ENGLISH_LEARNING_SCHEDULE_HOUR: {raw}"))?,
            Err(_) => 9,
        };
        if english_learning_schedule_hour > 23 {
            bail!("ENGLISH_LEARNING_SCHEDULE_HOUR must be between 0 and 23");
        }

        let english_learning_timezone_offset_hours =
            match std::env::var("ENGLISH_LEARNING_TZ_OFFSET_HOURS") {
                Ok(raw) => raw
                    .parse::<i32>()
                    .with_context(|| format!("invalid ENGLISH_LEARNING_TZ_OFFSET_HOURS: {raw}"))?,
                Err(_) => 8,
            };
        if !(-23..=23).contains(&english_learning_timezone_offset_hours) {
            bail!("ENGLISH_LEARNING_TZ_OFFSET_HOURS must be between -23 and 23");
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
            forms: FormConfig {
                markdown_dir: PathBuf::from(
                    std::env::var("FORM_MARKDOWN_DIR")
                        .unwrap_or_else(|_| "./forms".to_string()),
                ),
            },
            media_translate: MediaTranslateConfig {
                api_key: first_env(&[
                    "MEDIA_TRANSLATE_API_KEY",
                    "DASHSCOPE_API_KEY",
                    "BAILIAN_API_KEY",
                    "GLM_API_KEY",
                ]),
                base_url: first_env(&[
                    "MEDIA_TRANSLATE_BASE_URL",
                    "DASHSCOPE_BASE_URL",
                    "BAILIAN_BASE_URL",
                    "GLM_BASE_URL",
                ])
                .unwrap_or_else(|| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
                model: std::env::var("MEDIA_TRANSLATE_MODEL")
                    .unwrap_or_else(|_| "qwen3-livetranslate-flash".to_string()),
            },
            english_learning: EnglishLearningConfig {
                enabled: parse_bool_env("ENGLISH_LEARNING_ENABLED", true).with_context(|| {
                    "invalid ENGLISH_LEARNING_ENABLED, expected true/false".to_string()
                })?,
                scheduler_enabled: parse_bool_env("ENGLISH_LEARNING_SCHEDULER_ENABLED", true)
                    .with_context(|| {
                        "invalid ENGLISH_LEARNING_SCHEDULER_ENABLED, expected true/false"
                            .to_string()
                    })?,
                schedule_hour: english_learning_schedule_hour,
                timezone_offset_hours: english_learning_timezone_offset_hours,
                storage_dir: PathBuf::from(
                    std::env::var("ENGLISH_LEARNING_STORAGE_DIR")
                        .unwrap_or_else(|_| "./learning_data".to_string()),
                ),
                news_sources: {
                    let configured = parse_csv_env("ENGLISH_LEARNING_NEWS_SOURCES");
                    if configured.is_empty() {
                        vec!["https://feeds.bbci.co.uk/news/world/rss.xml".to_string()]
                    } else {
                        configured
                    }
                },
                max_feed_items_per_source: parse_usize_env(
                    "ENGLISH_LEARNING_MAX_FEED_ITEMS_PER_SOURCE",
                    5,
                )
                .with_context(|| {
                    "invalid ENGLISH_LEARNING_MAX_FEED_ITEMS_PER_SOURCE, expected integer"
                        .to_string()
                })?,
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
                audio_source_lang: std::env::var("FEISHU_AUDIO_SOURCE_LANG")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                audio_target_lang: std::env::var("FEISHU_AUDIO_TARGET_LANG")
                    .unwrap_or_else(|_| "zh".to_string()),
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
