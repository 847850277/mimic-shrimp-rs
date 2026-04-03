//! 应用配置模块，负责从环境变量装配整个服务运行所需的总配置。

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::{
    EnglishLearningConfig, ExecCommandToolConfig, FeishuCallbackConfig, FormConfig, LlmConfig,
    LlmProvider, MediaTranslateConfig, SpeechSynthesisConfig, WeixinChannelConfig,
    env::{
        first_env, parse_bool_env, parse_csv_env, parse_f32_env, parse_u64_env, parse_usize_env,
    },
};

/// 应用总配置，聚合了服务自身、模型、通道和工具相关配置。
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub server_addr: String,
    pub default_system_prompt: String,
    pub max_iterations: usize,
    pub max_context_messages: usize,
    pub llm: LlmConfig,
    pub forms: FormConfig,
    pub media_translate: MediaTranslateConfig,
    pub speech_synthesis: SpeechSynthesisConfig,
    pub english_learning: EnglishLearningConfig,
    pub feishu_callback: FeishuCallbackConfig,
    pub weixin_channel: WeixinChannelConfig,
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

        let max_context_messages = match std::env::var("MAX_CONTEXT_MESSAGES") {
            Ok(raw) => raw
                .parse::<usize>()
                .with_context(|| format!("invalid MAX_CONTEXT_MESSAGES: {raw}"))?,
            Err(_) => 20,
        };
        if max_context_messages == 0 {
            bail!("MAX_CONTEXT_MESSAGES must be greater than 0");
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
            app_name: std::env::var("APP_NAME")
                .unwrap_or_else(|_| "mimic-shrimp-rs".to_string()),
            server_addr: std::env::var("SERVER_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:7878".to_string()),
            default_system_prompt: std::env::var("SYSTEM_PROMPT").unwrap_or_else(|_| {
                "You are a tool-calling assistant inspired by OpenClaw. When deterministic work or external state is needed, call the available tools first, then synthesize a concise final answer.".to_string()
            }),
            max_iterations,
            max_context_messages,
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
            speech_synthesis: SpeechSynthesisConfig {
                api_key: first_env(&["SPEECH_SYNTHESIS_API_KEY", "SILICONFLOW_API_KEY"]),
                base_url: first_env(&["SPEECH_SYNTHESIS_BASE_URL", "SILICONFLOW_BASE_URL"])
                    .unwrap_or_else(|| "https://api.siliconflow.cn/v1".to_string()),
                model: std::env::var("SPEECH_SYNTHESIS_MODEL")
                    .unwrap_or_else(|_| "MOSS-TTSD-v0.5".to_string()),
                voice: std::env::var("SPEECH_SYNTHESIS_VOICE")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some("alex".to_string())),
                response_format: std::env::var("SPEECH_SYNTHESIS_RESPONSE_FORMAT")
                    .unwrap_or_else(|_| "mp3".to_string()),
                sample_rate: match std::env::var("SPEECH_SYNTHESIS_SAMPLE_RATE") {
                    Ok(raw) => Some(
                        raw.parse::<u32>().with_context(|| {
                            format!("invalid SPEECH_SYNTHESIS_SAMPLE_RATE: {raw}")
                        })?,
                    ),
                    Err(_) => None,
                },
                speed: parse_f32_env("SPEECH_SYNTHESIS_SPEED", 1.0).with_context(|| {
                    "invalid SPEECH_SYNTHESIS_SPEED, expected float".to_string()
                })?,
                gain: parse_f32_env("SPEECH_SYNTHESIS_GAIN", 0.0).with_context(|| {
                    "invalid SPEECH_SYNTHESIS_GAIN, expected float".to_string()
                })?,
                stream: parse_bool_env("SPEECH_SYNTHESIS_STREAM", false).with_context(|| {
                    "invalid SPEECH_SYNTHESIS_STREAM, expected true/false".to_string()
                })?,
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
            weixin_channel: WeixinChannelConfig {
                enabled: parse_bool_env("WEIXIN_ENABLED", false)
                    .with_context(|| "invalid WEIXIN_ENABLED, expected true/false".to_string())?,
                base_url: std::env::var("WEIXIN_API_BASE_URL")
                    .unwrap_or_else(|_| "https://ilinkai.weixin.qq.com".to_string()),
                cdn_base_url: std::env::var("WEIXIN_CDN_BASE_URL")
                    .unwrap_or_else(|_| "https://novac2c.cdn.weixin.qq.com/c2c".to_string()),
                state_dir: PathBuf::from(
                    std::env::var("WEIXIN_STATE_DIR")
                        .unwrap_or_else(|_| "./channel_state/weixin".to_string()),
                ),
                route_tag: std::env::var("WEIXIN_ROUTE_TAG")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                ilink_app_id: std::env::var("WEIXIN_ILINK_APP_ID")
                    .unwrap_or_else(|_| "bot".to_string()),
                bot_type: std::env::var("WEIXIN_BOT_TYPE")
                    .unwrap_or_else(|_| "3".to_string()),
                login_timeout_ms: parse_u64_env("WEIXIN_LOGIN_TIMEOUT_MS", 480_000)
                    .with_context(|| {
                        "invalid WEIXIN_LOGIN_TIMEOUT_MS, expected integer".to_string()
                    })?,
                long_poll_timeout_ms: parse_u64_env("WEIXIN_LONG_POLL_TIMEOUT_MS", 35_000)
                    .with_context(|| {
                        "invalid WEIXIN_LONG_POLL_TIMEOUT_MS, expected integer".to_string()
                    })?,
                retry_delay_ms: parse_u64_env("WEIXIN_RETRY_DELAY_MS", 2_000).with_context(
                    || "invalid WEIXIN_RETRY_DELAY_MS, expected integer".to_string(),
                )?,
                backoff_delay_ms: parse_u64_env("WEIXIN_BACKOFF_DELAY_MS", 30_000)
                    .with_context(|| {
                        "invalid WEIXIN_BACKOFF_DELAY_MS, expected integer".to_string()
                    })?,
                session_pause_minutes: parse_u64_env("WEIXIN_SESSION_PAUSE_MINUTES", 60)
                    .with_context(|| {
                        "invalid WEIXIN_SESSION_PAUSE_MINUTES, expected integer".to_string()
                    })?,
                supervisor_interval_ms: parse_u64_env("WEIXIN_SUPERVISOR_INTERVAL_MS", 30_000)
                    .with_context(|| {
                        "invalid WEIXIN_SUPERVISOR_INTERVAL_MS, expected integer".to_string()
                    })?,
                supervisor_stale_after_ms: parse_u64_env("WEIXIN_SUPERVISOR_STALE_AFTER_MS", 0)
                    .with_context(|| {
                        "invalid WEIXIN_SUPERVISOR_STALE_AFTER_MS, expected integer".to_string()
                    })?,
                supervisor_restart_gap_ms: parse_u64_env(
                    "WEIXIN_SUPERVISOR_RESTART_GAP_MS",
                    0,
                )
                .with_context(|| {
                    "invalid WEIXIN_SUPERVISOR_RESTART_GAP_MS, expected integer".to_string()
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
