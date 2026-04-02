//! 微信通道管理与消息编排。

use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use qrcodegen::{QrCode, QrCodeEcc};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
    time::{Duration, sleep},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    capability::{
        CapabilityHub, ConversationCapability, ConversationRequest, EnglishLearningCapability,
    },
    channel::{ChannelKind, InboundTextMessage},
    config::WeixinChannelConfig,
    logging,
};

use super::{
    api::WeixinApiClient,
    monitor::run_account_monitor,
    store::WeixinStore,
    types::{
        WeixinAccountRecord, WeixinAccountSummary, WeixinActiveLogin, WeixinLoginStartResult,
        WeixinLoginWaitResult, WeixinMessage,
    },
};

pub(crate) const SESSION_EXPIRED_ERRCODE: i64 = -14;
const LOGIN_TTL_MS: u64 = 5 * 60_000;
const MIN_STALE_AFTER_MS: u64 = 120_000;
const STALE_WINDOW_MULTIPLIER: u64 = 4;
const MIN_RESTART_GAP_MS: u64 = 60_000;

/// 微信通道管理器。
#[derive(Clone)]
pub struct WeixinManager {
    inner: Arc<WeixinManagerInner>,
}

struct WeixinManagerInner {
    config: WeixinChannelConfig,
    capabilities: CapabilityHub,
    api: WeixinApiClient,
    store: WeixinStore,
    login_sessions: Mutex<HashMap<String, WeixinActiveLogin>>,
    context_tokens: RwLock<HashMap<String, HashMap<String, String>>>,
    monitor_tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    supervisor_task: Mutex<Option<JoinHandle<()>>>,
}

impl WeixinManager {
    /// 创建一个新的微信通道管理器。
    pub fn new(config: WeixinChannelConfig, capabilities: CapabilityHub) -> Self {
        let api = WeixinApiClient::new(config.clone());
        let store = WeixinStore::new(config.state_dir.clone());
        Self {
            inner: Arc::new(WeixinManagerInner {
                config,
                capabilities,
                api,
                store,
                login_sessions: Mutex::new(HashMap::new()),
                context_tokens: RwLock::new(HashMap::new()),
                monitor_tasks: Mutex::new(HashMap::new()),
                supervisor_task: Mutex::new(None),
            }),
        }
    }

    /// 是否启用微信通道。
    pub fn is_enabled(&self) -> bool {
        self.inner.config.enabled
    }

    /// 返回只读配置。
    pub(crate) fn config(&self) -> &WeixinChannelConfig {
        &self.inner.config
    }

    /// 返回协议客户端。
    pub(crate) fn api(&self) -> &WeixinApiClient {
        &self.inner.api
    }

    /// 启动全部已保存账号的长轮询任务。
    pub async fn start_all_monitors(&self) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        for account in self.inner.store.load_accounts()? {
            self.load_context_tokens(account.account_id.as_str())
                .await?;
            self.start_monitor(account).await?;
        }
        Ok(())
    }

    /// 启动微信账号在线保持 supervisor。
    pub async fn start_supervisor(&self) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        let mut task = self.inner.supervisor_task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return Ok(());
        }
        let manager = self.clone();
        *task = Some(tokio::spawn(async move {
            loop {
                if let Err(error) = manager.run_supervisor_pass().await {
                    warn!(error = %error, "weixin supervisor pass failed");
                }
                sleep(Duration::from_millis(
                    manager.inner.config.supervisor_interval_ms.max(5_000),
                ))
                .await;
            }
        }));
        Ok(())
    }

    /// 开始二维码登录。
    pub async fn start_login(
        &self,
        force: bool,
        account_id: Option<String>,
    ) -> Result<WeixinLoginStartResult> {
        if !self.is_enabled() {
            return Err(anyhow!("WEIXIN_ENABLED is false"));
        }
        let session_key = account_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        if !force {
            let sessions = self.inner.login_sessions.lock().await;
            if let Some(existing) = sessions.get(session_key.as_str()) {
                if now_ms().saturating_sub(existing.started_at_ms) < LOGIN_TTL_MS {
                    return Ok(WeixinLoginStartResult {
                        session_key,
                        qr_code_url: Some(existing.qrcode_url.clone()),
                        qr_code_data_url: build_qr_code_data_url(existing.qrcode_url.as_str()),
                        message: "二维码已就绪，请使用微信扫描。".to_string(),
                    });
                }
            }
        }

        let qr = self
            .inner
            .api
            .fetch_qr_code(self.inner.config.bot_type.as_str())
            .await?;
        let mut sessions = self.inner.login_sessions.lock().await;
        sessions.insert(
            session_key.clone(),
            WeixinActiveLogin {
                qrcode: qr.qrcode,
                qrcode_url: qr.qrcode_img_content.clone(),
                started_at_ms: now_ms(),
                current_api_base_url: self.inner.config.base_url.clone(),
            },
        );
        Ok(WeixinLoginStartResult {
            session_key,
            qr_code_data_url: build_qr_code_data_url(qr.qrcode_img_content.as_str()),
            qr_code_url: Some(qr.qrcode_img_content),
            message: "使用微信扫描二维码完成连接。".to_string(),
        })
    }

    /// 等待二维码登录结果。
    pub async fn wait_login(
        &self,
        session_key: &str,
        timeout_ms: Option<u64>,
    ) -> Result<WeixinLoginWaitResult> {
        if !self.is_enabled() {
            return Err(anyhow!("WEIXIN_ENABLED is false"));
        }
        let timeout_ms = timeout_ms
            .unwrap_or(self.inner.config.login_timeout_ms)
            .max(1_000);
        let deadline = now_ms() + timeout_ms;

        loop {
            let login = {
                let sessions = self.inner.login_sessions.lock().await;
                let Some(login) = sessions.get(session_key) else {
                    return Ok(WeixinLoginWaitResult {
                        connected: false,
                        account_id: None,
                        linked_user_id: None,
                        message: "当前没有进行中的微信登录会话。".to_string(),
                    });
                };
                login.clone()
            };
            if now_ms().saturating_sub(login.started_at_ms) >= LOGIN_TTL_MS {
                let mut sessions = self.inner.login_sessions.lock().await;
                sessions.remove(session_key);
                return Ok(WeixinLoginWaitResult {
                    connected: false,
                    account_id: None,
                    linked_user_id: None,
                    message: "二维码已过期，请重新生成。".to_string(),
                });
            }
            let status = self
                .inner
                .api
                .poll_qr_status(
                    login.current_api_base_url.as_str(),
                    login.qrcode.as_str(),
                    self.inner.config.long_poll_timeout_ms,
                )
                .await?;
            match status.status.as_str() {
                "wait" | "scaned" => {}
                "scaned_but_redirect" => {
                    if let Some(host) = status
                        .redirect_host
                        .as_deref()
                        .filter(|value| !value.is_empty())
                    {
                        let mut sessions = self.inner.login_sessions.lock().await;
                        if let Some(active) = sessions.get_mut(session_key) {
                            active.current_api_base_url = format!("https://{}", host);
                        }
                    }
                }
                "expired" => {
                    let mut sessions = self.inner.login_sessions.lock().await;
                    sessions.remove(session_key);
                    return Ok(WeixinLoginWaitResult {
                        connected: false,
                        account_id: None,
                        linked_user_id: None,
                        message: "二维码已过期，请重新生成。".to_string(),
                    });
                }
                "confirmed" => {
                    let account_id = status
                        .ilink_bot_id
                        .clone()
                        .ok_or_else(|| anyhow!("login confirmed but ilink_bot_id is missing"))?;
                    let bot_token = status
                        .bot_token
                        .clone()
                        .ok_or_else(|| anyhow!("login confirmed but bot_token is missing"))?;
                    let account = WeixinAccountRecord {
                        account_id: account_id.clone(),
                        bot_token,
                        base_url: status
                            .baseurl
                            .unwrap_or_else(|| self.inner.config.base_url.clone()),
                        linked_user_id: status.ilink_user_id.clone(),
                        saved_at_ms: now_ms(),
                    };
                    self.inner.store.save_account(&account)?;
                    self.load_context_tokens(account.account_id.as_str())
                        .await?;
                    let mut sessions = self.inner.login_sessions.lock().await;
                    sessions.remove(session_key);
                    self.start_monitor(account).await?;
                    return Ok(WeixinLoginWaitResult {
                        connected: true,
                        account_id: Some(account_id),
                        linked_user_id: status.ilink_user_id,
                        message: "与微信连接成功。".to_string(),
                    });
                }
                other => {
                    return Err(anyhow!("unsupported qr login status: {other}"));
                }
            }
            if now_ms() >= deadline {
                return Ok(WeixinLoginWaitResult {
                    connected: false,
                    account_id: None,
                    linked_user_id: None,
                    message: "等待扫码确认超时，请继续使用当前二维码或重新生成。".to_string(),
                });
            }
            sleep(Duration::from_millis(1_000)).await;
        }
    }

    /// 列出当前账号。
    pub async fn list_accounts(&self) -> Result<Vec<WeixinAccountSummary>> {
        self.inner.store.list_accounts().await
    }

    /// 重启一个账号的监控任务。
    pub async fn restart_account(&self, account_id: &str) -> Result<()> {
        let account = self
            .inner
            .store
            .load_account(account_id)?
            .ok_or_else(|| anyhow!("weixin account not found: {account_id}"))?;
        {
            let mut tasks = self.inner.monitor_tasks.lock().await;
            if let Some(handle) = tasks.remove(account_id) {
                handle.abort();
            }
        }
        self.mark_runtime_stopped(account_id).await;
        self.load_context_tokens(account_id).await?;
        self.start_monitor(account).await
    }

    /// 处理收到的一条微信消息。
    pub(crate) async fn handle_incoming_message(
        &self,
        account: &WeixinAccountRecord,
        message: WeixinMessage,
    ) -> Result<()> {
        let Some(from_user_id) = message
            .from_user_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        let Some(text) = extract_message_text(&message).filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        if let Some(token) = message
            .context_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            self.set_context_token(account.account_id.as_str(), from_user_id, token)
                .await?;
        }

        let inbound = InboundTextMessage {
            channel: ChannelKind::Weixin,
            event_id: None,
            message_id: message
                .message_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            chat_id: None,
            chat_type: Some("direct".to_string()),
            user_id: from_user_id.to_string(),
            session_id: format!("weixin:{}:{}", account.account_id, from_user_id),
            text: text.clone(),
        };
        logging::log_channel_text_message_received(
            inbound.channel.as_str(),
            inbound.event_id.as_deref(),
            &inbound.message_id,
            inbound.chat_id.as_deref(),
            inbound.chat_type.as_deref(),
            &inbound.session_id,
            &inbound.user_id,
            &inbound.text,
        );

        let answer = build_text_reply(
            self.inner.capabilities.conversation(),
            self.inner.capabilities.english_learning(),
            &inbound.session_id,
            &inbound.user_id,
            &inbound.text,
        )
        .await?;
        let context_token = message
            .context_token
            .clone()
            .or_else(|| self.get_context_token(account.account_id.as_str(), from_user_id));
        let _message_id = self
            .inner
            .api
            .send_text_message(
                account,
                from_user_id,
                answer.as_str(),
                context_token.as_deref(),
            )
            .await?;
        info!(
            account_id = %account.account_id,
            peer = %from_user_id,
            session_id = %inbound.session_id,
            "weixin text reply sent"
        );
        Ok(())
    }

    pub(crate) async fn mark_runtime_started(&self, account_id: &str) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                state.running = true;
                state.last_start_at_ms = Some(now_ms());
                state.paused_until_ms = None;
                state.last_error = None;
            })
            .await;
    }

    pub(crate) async fn mark_runtime_event(&self, account_id: &str, inbound: bool) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                let now = now_ms();
                state.last_event_at_ms = Some(now);
                state.paused_until_ms = None;
                state.last_error = None;
                if inbound {
                    state.last_inbound_at_ms = Some(now);
                }
            })
            .await;
    }

    pub(crate) async fn mark_runtime_error(&self, account_id: &str, message: String) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                state.last_error = Some(message);
            })
            .await;
    }

    pub(crate) async fn mark_runtime_paused_until(&self, account_id: &str, paused_until_ms: u64) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                state.paused_until_ms = Some(paused_until_ms);
            })
            .await;
    }

    pub(crate) async fn mark_runtime_stopped(&self, account_id: &str) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                state.running = false;
            })
            .await;
    }

    async fn mark_runtime_restart_requested(&self, account_id: &str, reason: &str) {
        self.inner
            .store
            .mutate_runtime(account_id, |state| {
                let now = now_ms();
                state.last_restart_at_ms = Some(now);
                state.last_error = Some(format!("supervisor restart requested: {reason}"));
            })
            .await;
    }

    pub(crate) fn load_sync_cursor(&self, account_id: &str) -> Result<Option<String>> {
        self.inner.store.load_sync_cursor(account_id)
    }

    pub(crate) fn save_sync_cursor(&self, account_id: &str, cursor: &str) -> Result<()> {
        self.inner.store.save_sync_cursor(account_id, cursor)
    }

    async fn start_monitor(&self, account: WeixinAccountRecord) -> Result<()> {
        let account_id = account.account_id.clone();
        let mut tasks = self.inner.monitor_tasks.lock().await;
        if let Some(handle) = tasks.get(account_id.as_str()) {
            if !handle.is_finished() {
                return Ok(());
            }
        }
        let manager = self.clone();
        let runtime_account_id = account_id.clone();
        let handle = tokio::spawn(async move {
            run_account_monitor(manager.clone(), account).await;
            manager
                .mark_runtime_stopped(runtime_account_id.as_str())
                .await;
        });
        tasks.insert(account_id, handle);
        Ok(())
    }

    async fn run_supervisor_pass(&self) -> Result<()> {
        for account in self.inner.store.load_accounts()? {
            if let Some(reason) = self
                .supervisor_restart_reason(account.account_id.as_str())
                .await
            {
                info!(
                    account_id = %account.account_id,
                    reason = %reason,
                    "weixin supervisor restarting account monitor"
                );
                self.mark_runtime_restart_requested(account.account_id.as_str(), &reason)
                    .await;
                self.restart_account(account.account_id.as_str()).await?;
            }
        }
        Ok(())
    }

    async fn supervisor_restart_reason(&self, account_id: &str) -> Option<String> {
        let runtime = self.inner.store.load_runtime_state(account_id).await;
        let now = now_ms();

        if let Some(paused_until_ms) = runtime.paused_until_ms {
            if now < paused_until_ms {
                return None;
            }
        }

        if !self.monitor_task_running(account_id).await {
            return Some("monitor task is missing or already finished".to_string());
        }

        let stale_after_ms = self.supervisor_stale_after_ms();
        let reference_ms = runtime.last_event_at_ms.or(runtime.last_start_at_ms);
        let Some(reference_ms) = reference_ms else {
            return None;
        };

        if now.saturating_sub(reference_ms) < stale_after_ms {
            return None;
        }

        if let Some(last_restart_at_ms) = runtime.last_restart_at_ms {
            if now.saturating_sub(last_restart_at_ms) < self.supervisor_restart_gap_ms() {
                return None;
            }
        }

        Some(format!(
            "stale monitor heartbeat for {} ms",
            now.saturating_sub(reference_ms)
        ))
    }

    async fn monitor_task_running(&self, account_id: &str) -> bool {
        let mut tasks = self.inner.monitor_tasks.lock().await;
        let finished = tasks
            .get(account_id)
            .map(|handle| handle.is_finished())
            .unwrap_or(true);
        if finished {
            tasks.remove(account_id);
            false
        } else {
            true
        }
    }

    fn supervisor_stale_after_ms(&self) -> u64 {
        if self.inner.config.supervisor_stale_after_ms > 0 {
            return self.inner.config.supervisor_stale_after_ms;
        }
        (self.inner.config.long_poll_timeout_ms * STALE_WINDOW_MULTIPLIER).max(MIN_STALE_AFTER_MS)
    }

    fn supervisor_restart_gap_ms(&self) -> u64 {
        if self.inner.config.supervisor_restart_gap_ms > 0 {
            return self.inner.config.supervisor_restart_gap_ms;
        }
        self.inner.config.backoff_delay_ms.max(MIN_RESTART_GAP_MS)
    }

    async fn load_context_tokens(&self, account_id: &str) -> Result<()> {
        let tokens = self.inner.store.load_context_tokens(account_id)?;
        let mut cache = self.inner.context_tokens.write().await;
        cache.insert(account_id.to_string(), tokens);
        Ok(())
    }

    async fn set_context_token(&self, account_id: &str, user_id: &str, token: &str) -> Result<()> {
        let snapshot = {
            let mut cache = self.inner.context_tokens.write().await;
            let entry = cache.entry(account_id.to_string()).or_default();
            entry.insert(user_id.to_string(), token.to_string());
            entry.clone()
        };
        self.inner.store.save_context_tokens(account_id, &snapshot)
    }

    fn get_context_token(&self, account_id: &str, user_id: &str) -> Option<String> {
        self.inner.context_tokens.try_read().ok().and_then(|cache| {
            cache
                .get(account_id)
                .and_then(|tokens| tokens.get(user_id))
                .cloned()
        })
    }
}

fn extract_message_text(message: &WeixinMessage) -> Option<String> {
    for item in &message.item_list {
        if item.item_type == 1 {
            if let Some(text) = item
                .text_item
                .as_ref()
                .and_then(|value| value.text.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Some(text.to_string());
            }
        }
        if item.item_type == 3 {
            if let Some(text) = item
                .voice_item
                .as_ref()
                .and_then(|value| value.text.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return Some(text.to_string());
            }
        }
    }
    None
}

async fn build_text_reply(
    conversation: &ConversationCapability,
    english_learning: &EnglishLearningCapability,
    session_id: &str,
    user_id: &str,
    message: &str,
) -> Result<String> {
    if let Some(reply) = english_learning
        .maybe_handle_message(session_id, message)
        .await?
    {
        let trimmed = reply.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let response = conversation
        .execute(ConversationRequest {
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            message: message.to_string(),
            system_prompt: None,
            max_iterations: None,
            persist: true,
        })
        .await?;

    if response.answer.trim().is_empty() {
        Ok("我暂时还没有合适的回复，请稍后再试。".to_string())
    } else {
        Ok(response.answer)
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn build_qr_code_data_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let svg = render_qr_svg(&qr, 8);
    Some(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg.as_bytes())
    ))
}

fn render_qr_svg(qr: &QrCode, border: i32) -> String {
    let size = qr.size();
    let dimension = size + border * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dimension} {dimension}\" shape-rendering=\"crispEdges\"><rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/><path d=\"{path}\" fill=\"#111111\"/></svg>"
    )
}
