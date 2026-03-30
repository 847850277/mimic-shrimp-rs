//! Web 状态模块，负责保存全局 HTTP 状态以及把共享状态注入到每个请求上下文中。

use std::sync::Arc;

use async_trait::async_trait;
use salvo::{Depot, FlowCtrl, Handler, Request, Response};

use crate::{
    capability::CapabilityHub, channel::weixin::WeixinManager, config::AppConfig,
    forms::SharedFormDefinitionStore,
};

/// HTTP 层共享状态，包含配置和对外暴露的能力集合。
#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub capabilities: CapabilityHub,
    pub form_store: SharedFormDefinitionStore,
    pub weixin_manager: Arc<WeixinManager>,
}

/// 状态注入器，会在每个请求开始时把 `AppState` 放入 `Depot`。
#[derive(Clone)]
pub(crate) struct StateInjector {
    pub(crate) state: Arc<AppState>,
}

#[async_trait]
impl Handler for StateInjector {
    /// 执行状态注入并继续后续 handler 流程。
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        depot.inject(self.state.clone());
        ctrl.call_next(req, depot, res).await;
    }
}

/// 从 `Depot` 中取回当前请求关联的 `AppState`。
pub(crate) fn app_state(depot: &Depot) -> Arc<AppState> {
    match depot.obtain::<Arc<AppState>>() {
        Ok(state) => state.clone(),
        Err(_) => panic!("app state is missing"),
    }
}
