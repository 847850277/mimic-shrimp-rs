//! Web 路由模块，负责启动 HTTP 服务并组装所有路由。

use std::sync::Arc;

use anyhow::Result;
use salvo::{
    Listener, Router,
    prelude::{Server, TcpListener},
};

use super::{
    handlers::{
        chat, extract_form, feishu_callback, health, invoke_tool, list_sessions, list_tools,
        session_history,
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
        .hoop(StateInjector { state })
        .push(Router::with_path("health").get(health))
        .push(
            Router::with_path("feishu/callback")
                .get(feishu_callback)
                .post(feishu_callback),
        )
        .push(
            Router::with_path("api/feishu/callback")
                .get(feishu_callback)
                .post(feishu_callback),
        )
        .push(Router::with_path("tools").get(list_tools))
        .push(Router::with_path("tools/invoke").post(invoke_tool))
        .push(Router::with_path("chat").post(chat))
        .push(Router::with_path("extract/form").post(extract_form))
        .push(Router::with_path("sessions").get(list_sessions))
        .push(Router::with_path("sessions/{session_id}/history").get(session_history))
}
