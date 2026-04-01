//! Web 路由模块，负责启动 HTTP 服务并组装所有路由。

use std::sync::Arc;

use anyhow::Result;
use salvo::{
    Listener, Router,
    prelude::{Server, TcpListener},
};

use super::{
    cors::CorsHandler,
    handlers::{
        chat, cors_preflight, extract_form, feishu_callback, health, invoke_tool, list_sessions,
        list_tools, list_weixin_accounts, session_history, synthesize_speech, translate_media,
        weixin_connect_page, weixin_login_start, weixin_login_wait, weixin_restart_account,
    },
    state::{AppState, StateInjector},
};

/// 启动 HTTP 服务并监听配置中的地址。
pub async fn run_http(state: Arc<AppState>) -> Result<()> {
    let router = build_router(state.clone());
    let acceptor = TcpListener::new(state.config.server_addr.clone())
        .bind()
        .await;
    Server::new(acceptor).serve(router).await;
    Ok(())
}

/// 构建应用的全部 HTTP 路由树。
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .hoop(CorsHandler)
        .hoop(StateInjector { state })
        .push(
            Router::with_path("health")
                .options(cors_preflight)
                .get(health),
        )
        .push(
            Router::with_path("feishu/callback")
                .options(cors_preflight)
                .get(feishu_callback)
                .post(feishu_callback),
        )
        .push(
            Router::with_path("api/feishu/callback")
                .options(cors_preflight)
                .get(feishu_callback)
                .post(feishu_callback),
        )
        .push(
            Router::with_path("weixin/connect")
                .options(cors_preflight)
                .get(weixin_connect_page),
        )
        .push(
            Router::with_path("weixin/login/start")
                .options(cors_preflight)
                .post(weixin_login_start),
        )
        .push(
            Router::with_path("weixin/login/wait")
                .options(cors_preflight)
                .post(weixin_login_wait),
        )
        .push(
            Router::with_path("weixin/accounts")
                .options(cors_preflight)
                .get(list_weixin_accounts),
        )
        .push(
            Router::with_path("weixin/accounts/{account_id}/restart")
                .options(cors_preflight)
                .post(weixin_restart_account),
        )
        .push(
            Router::with_path("tools")
                .options(cors_preflight)
                .get(list_tools),
        )
        .push(
            Router::with_path("tools/invoke")
                .options(cors_preflight)
                .post(invoke_tool),
        )
        .push(Router::with_path("chat").options(cors_preflight).post(chat))
        .push(
            Router::with_path("extract/form")
                .options(cors_preflight)
                .post(extract_form),
        )
        .push(
            Router::with_path("translate/media")
                .options(cors_preflight)
                .post(translate_media),
        )
        .push(
            Router::with_path("speech/synthesize")
                .options(cors_preflight)
                .post(synthesize_speech),
        )
        .push(
            Router::with_path("sessions")
                .options(cors_preflight)
                .get(list_sessions),
        )
        .push(
            Router::with_path("sessions/{session_id}/history")
                .options(cors_preflight)
                .get(session_history),
        )
}
