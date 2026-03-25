//! 程序入口模块，负责初始化配置、构建依赖并启动 HTTP 服务。

mod capability;
mod channel;
mod config;
mod engine;
mod forms;
mod logging;
mod models;
mod session_store;
mod tools;
mod web;

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt};

use crate::{
    capability::{CapabilityHub, EnglishLearningCapability, StructuredExtractionCapability},
    config::AppConfig,
    engine::ToolCallEngine,
    forms::{MarkdownFormStore, SharedFormDefinitionStore},
    models::build_llm,
    session_store::SessionStore,
    tools::build_builtin_registry,
    web::{AppState, run_http},
};

#[tokio::main]
/// 初始化应用主流程并启动服务。
async fn main() -> Result<()> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let llm = build_llm(&config.llm)?;
    let extraction_llm = llm.clone();
    let extraction = StructuredExtractionCapability::new(extraction_llm);
    let english_learning =
        EnglishLearningCapability::new(config.english_learning.clone(), extraction.clone());
    let form_store: SharedFormDefinitionStore =
        Arc::new(MarkdownFormStore::new(config.forms.markdown_dir.clone()));
    let session_store = SessionStore::default();
    let registry = build_builtin_registry(
        session_store.clone(),
        config.exec_command_tool.clone(),
        Some(english_learning.clone()),
    )?;
    let engine = Arc::new(ToolCallEngine::new(
        config.app_name.clone(),
        llm,
        registry,
        session_store,
        config.default_system_prompt.clone(),
        config.max_iterations,
        config.max_context_messages,
    ));
    let capabilities = CapabilityHub::new(
        engine,
        extraction,
        config.media_translate.clone(),
        config.speech_synthesis.clone(),
        english_learning,
    );

    logging::log_service_startup(
        &config.server_addr,
        config.llm.provider.as_str(),
        &config.llm.model,
    );
    logging::log_feishu_integration_config(
        &config.feishu_callback.open_base_url,
        config.feishu_callback.verification_token.is_some(),
        config.feishu_callback.encrypt_key.is_some(),
        config.feishu_callback.app_id.is_some(),
        config.feishu_callback.app_secret.is_some(),
        config.feishu_callback.require_mention,
        config.exec_command_tool.enabled,
        &config.exec_command_tool.shell,
    );
    logging::log_learning_config(
        config.english_learning.enabled,
        config.english_learning.scheduler_enabled,
        config.english_learning.schedule_hour,
        config.english_learning.timezone_offset_hours,
        &config.english_learning.storage_dir,
        config.english_learning.news_sources.len(),
    );
    if config.feishu_callback.app_id.is_none() || config.feishu_callback.app_secret.is_none() {
        logging::log_feishu_reply_disabled();
    }

    if config.english_learning.enabled && config.english_learning.scheduler_enabled {
        let learning = capabilities.english_learning().clone();
        tokio::spawn(async move {
            learning.run_scheduler_loop().await;
        });
    }

    run_http(Arc::new(AppState {
        config,
        capabilities,
        form_store,
    }))
    .await
}

/// 初始化全局 tracing 日志配置。
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rs_tool_call=debug"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .compact()
        .init();
}
