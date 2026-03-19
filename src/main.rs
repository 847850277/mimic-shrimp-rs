mod config;
mod engine;
mod feishu_bot;
mod feishu_callback;
mod http_api;
mod logging;
mod models;
mod session_store;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt};

use crate::{
    config::AppConfig,
    engine::ToolCallEngine,
    http_api::{AppState, run_http},
    models::build_llm,
    session_store::SessionStore,
    tools::build_builtin_registry,
};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let llm = build_llm(&config.llm)?;
    let session_store = SessionStore::default();
    let registry = build_builtin_registry(session_store.clone(), config.exec_command_tool.clone())?;
    let engine = Arc::new(ToolCallEngine::new(
        config.app_name.clone(),
        llm,
        registry,
        session_store,
        config.default_system_prompt.clone(),
        config.max_iterations,
    ));

    tracing::info!(
        server_addr = %config.server_addr,
        provider = %config.llm.provider.as_str(),
        model = %config.llm.model,
        "starting tool-call service"
    );
    tracing::info!(
        feishu_open_base_url = %config.feishu_callback.open_base_url,
        feishu_verification_token_configured = config.feishu_callback.verification_token.is_some(),
        feishu_encrypt_key_configured = config.feishu_callback.encrypt_key.is_some(),
        feishu_app_id_configured = config.feishu_callback.app_id.is_some(),
        feishu_app_secret_configured = config.feishu_callback.app_secret.is_some(),
        feishu_require_mention = config.feishu_callback.require_mention,
        exec_command_tool_enabled = config.exec_command_tool.enabled,
        exec_command_tool_shell = %config.exec_command_tool.shell,
        "loaded feishu integration config"
    );
    if config.feishu_callback.app_id.is_none() || config.feishu_callback.app_secret.is_none() {
        tracing::warn!(
            "feishu message reply is disabled because FEISHU_APP_ID or FEISHU_APP_SECRET is missing"
        );
    }

    run_http(Arc::new(AppState { config, engine })).await
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rs_tool_call=debug"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
