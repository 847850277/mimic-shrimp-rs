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
        SpeechSynthesisCapability, SpeechSynthesisRequest,
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
        let is_voice_message = message_contains_voice_item(&message);
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

        let answer = if is_voice_message {
            build_audio_reply(
                self.inner.capabilities.conversation(),
                self.inner.capabilities.english_learning(),
                &inbound.session_id,
                &inbound.user_id,
                &inbound.text,
            )
            .await?
        } else {
            build_text_reply(
                self.inner.capabilities.conversation(),
                self.inner.capabilities.english_learning(),
                &inbound.session_id,
                &inbound.user_id,
                &inbound.text,
            )
            .await?
        };
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
        if is_voice_message
            && self.inner.capabilities.speech_synthesis().is_configured()
            && should_send_english_media_reply(&inbound.text, &answer)
        {
            if let Err(error) = self
                .send_english_file_reply(
                    account,
                    from_user_id,
                    &inbound.message_id,
                    &inbound.session_id,
                    &answer,
                    context_token.as_deref(),
                )
                .await
            {
                logging::log_channel_background_error(
                    inbound.channel.as_str(),
                    &format!("failed to send synthesized weixin file reply: {error}"),
                );
            }
        }
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

    async fn send_english_file_reply(
        &self,
        account: &WeixinAccountRecord,
        to_user_id: &str,
        reply_to_message_id: &str,
        session_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> Result<()> {
        send_english_file_reply(
            self.inner.capabilities.speech_synthesis(),
            self.api(),
            account,
            to_user_id,
            reply_to_message_id,
            session_id,
            text,
            context_token,
        )
        .await
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

fn message_contains_voice_item(message: &WeixinMessage) -> bool {
    message.item_list.iter().any(|item| item.item_type == 3)
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

async fn build_audio_reply(
    conversation: &ConversationCapability,
    english_learning: &EnglishLearningCapability,
    session_id: &str,
    user_id: &str,
    transcript: &str,
) -> Result<String> {
    if let Some(reply) = english_learning
        .maybe_handle_message(session_id, transcript)
        .await?
    {
        let trimmed = reply.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(reply) = english_learning
        .maybe_handle_shadowing_audio(session_id, transcript)
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
            message: transcript.to_string(),
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

async fn send_english_file_reply(
    speech_synthesis: &SpeechSynthesisCapability,
    api: &WeixinApiClient,
    account: &WeixinAccountRecord,
    to_user_id: &str,
    reply_to_message_id: &str,
    session_id: &str,
    text: &str,
    context_token: Option<&str>,
) -> Result<()> {
    let normalized_text = normalize_text_for_speech(text);
    if normalized_text.is_empty() || !looks_like_english_text(&normalized_text) {
        return Ok(());
    }

    let file_name = format!("english-reply-{}.wav", now_ms());
    let format = "wav";
    let sample_rate = 32_000u32;
    logging::log_channel_media_reply_stage(
        ChannelKind::Weixin.as_str(),
        reply_to_message_id,
        "tts",
        &file_name,
        format,
        0,
        None,
    );
    let synthesized = speech_synthesis
        .execute(SpeechSynthesisRequest {
            text: normalized_text,
            model: None,
            voice: None,
            response_format: Some(format.to_string()),
            sample_rate: Some(sample_rate),
            speed: None,
            gain: None,
            stream: Some(false),
        })
        .await?;
    let audio_bytes = STANDARD
        .decode(&synthesized.audio_base64)
        .map_err(|error| anyhow::anyhow!("invalid synthesized audio base64: {error}"))?;
    let duration_ms = estimate_audio_duration_ms(format, &audio_bytes);
    logging::log_channel_media_reply_stage(
        ChannelKind::Weixin.as_str(),
        reply_to_message_id,
        "cdn_upload",
        &file_name,
        format,
        audio_bytes.len(),
        duration_ms,
    );
    let uploaded = api.upload_file(account, to_user_id, &audio_bytes).await?;
    logging::log_channel_media_reply_stage(
        ChannelKind::Weixin.as_str(),
        reply_to_message_id,
        "sendmessage",
        &file_name,
        format,
        audio_bytes.len(),
        duration_ms,
    );
    let _message_id = api
        .send_file_message(account, to_user_id, &file_name, &uploaded, context_token)
        .await?;
    logging::log_channel_media_replied(
        ChannelKind::Weixin.as_str(),
        reply_to_message_id,
        session_id,
        &file_name,
        format,
        duration_ms,
    );
    Ok(())
}

fn should_send_english_media_reply(transcript: &str, answer: &str) -> bool {
    looks_like_english_text(transcript)
        && looks_like_english_text(&normalize_text_for_speech(answer))
}

fn normalize_text_for_speech(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .replace("**", "")
        .replace("__", "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_english_text(input: &str) -> bool {
    let mut latin_letters = 0usize;
    let mut cjk_chars = 0usize;
    let mut words = 0usize;
    let mut in_word = false;

    for ch in input.chars() {
        if ch.is_ascii_alphabetic() {
            latin_letters += 1;
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
                cjk_chars += 1;
            }
        }
    }

    latin_letters >= 12 && words >= 3 && latin_letters > cjk_chars * 2
}

fn estimate_audio_duration_ms(format: &str, bytes: &[u8]) -> Option<u64> {
    match normalize_audio_format(format).as_str() {
        "mp3" => estimate_mp3_duration_ms(bytes),
        "wav" => estimate_wav_duration_ms(bytes),
        _ => None,
    }
}

fn normalize_audio_format(format: &str) -> String {
    match format.trim().to_ascii_lowercase().as_str() {
        "mpeg" => "mp3".to_string(),
        "wave" | "x-wav" => "wav".to_string(),
        other => other.to_string(),
    }
}

fn estimate_wav_duration_ms(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let byte_rate = u32::from_le_bytes(bytes[28..32].try_into().ok()?);
    let data_size = u32::from_le_bytes(bytes[40..44].try_into().ok()?);
    if byte_rate == 0 {
        return None;
    }
    Some(((u128::from(data_size) * 1000) / u128::from(byte_rate)) as u64)
}

fn estimate_mp3_duration_ms(bytes: &[u8]) -> Option<u64> {
    let mut offset = skip_id3v2_tag(bytes)?;
    let mut total_samples = 0u64;
    let mut fallback_sample_rate = None::<u32>;

    while offset + 4 <= bytes.len() {
        let header = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?);
        if (header >> 21) & 0x7ff != 0x7ff {
            offset += 1;
            continue;
        }
        let version_bits = (header >> 19) & 0x3;
        let layer_bits = (header >> 17) & 0x3;
        let bitrate_index = ((header >> 12) & 0xf) as usize;
        let sample_rate_index = ((header >> 10) & 0x3) as usize;
        let padding = ((header >> 9) & 0x1) as u32;

        let Some(version) = mpeg_version(version_bits) else {
            offset += 1;
            continue;
        };
        let Some(layer) = mpeg_layer(layer_bits) else {
            offset += 1;
            continue;
        };
        let Some(sample_rate) = mp3_sample_rate(version, sample_rate_index) else {
            offset += 1;
            continue;
        };
        let Some(bitrate_kbps) = mp3_bitrate_kbps(version, layer, bitrate_index) else {
            offset += 1;
            continue;
        };
        let frame_len = mp3_frame_length_bytes(version, layer, bitrate_kbps, sample_rate, padding)?;
        if frame_len == 0 || offset + frame_len > bytes.len() {
            break;
        }
        total_samples = total_samples.saturating_add(mp3_samples_per_frame(version, layer) as u64);
        fallback_sample_rate = Some(sample_rate);
        offset += frame_len;
    }

    let sample_rate = fallback_sample_rate?;
    if total_samples == 0 {
        return None;
    }
    Some(((u128::from(total_samples) * 1000) / u128::from(sample_rate)) as u64)
}

fn skip_id3v2_tag(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 10 {
        return Some(0);
    }
    if &bytes[..3] != b"ID3" {
        return Some(0);
    }
    let size = ((usize::from(bytes[6] & 0x7f)) << 21)
        | ((usize::from(bytes[7] & 0x7f)) << 14)
        | ((usize::from(bytes[8] & 0x7f)) << 7)
        | usize::from(bytes[9] & 0x7f);
    Some(10 + size)
}

#[derive(Clone, Copy)]
enum MpegVersion {
    V1,
    V2,
    V25,
}

#[derive(Clone, Copy)]
enum MpegLayer {
    L1,
    L2,
    L3,
}

fn mpeg_version(bits: u32) -> Option<MpegVersion> {
    match bits {
        0 => Some(MpegVersion::V25),
        2 => Some(MpegVersion::V2),
        3 => Some(MpegVersion::V1),
        _ => None,
    }
}

fn mpeg_layer(bits: u32) -> Option<MpegLayer> {
    match bits {
        1 => Some(MpegLayer::L3),
        2 => Some(MpegLayer::L2),
        3 => Some(MpegLayer::L1),
        _ => None,
    }
}

fn mp3_sample_rate(version: MpegVersion, index: usize) -> Option<u32> {
    const V1: [u32; 3] = [44_100, 48_000, 32_000];
    const V2: [u32; 3] = [22_050, 24_000, 16_000];
    const V25: [u32; 3] = [11_025, 12_000, 8_000];
    let table = match version {
        MpegVersion::V1 => &V1,
        MpegVersion::V2 => &V2,
        MpegVersion::V25 => &V25,
    };
    table.get(index).copied()
}

fn mp3_bitrate_kbps(version: MpegVersion, layer: MpegLayer, index: usize) -> Option<u32> {
    if index == 0 || index == 15 {
        return None;
    }
    const MPEG1_L1: [u32; 14] = [
        32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ];
    const MPEG1_L2: [u32; 14] = [
        32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ];
    const MPEG1_L3: [u32; 14] = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const MPEG2_L1: [u32; 14] = [
        32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ];
    const MPEG2_L23: [u32; 14] = [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

    let table = match (version, layer) {
        (MpegVersion::V1, MpegLayer::L1) => &MPEG1_L1,
        (MpegVersion::V1, MpegLayer::L2) => &MPEG1_L2,
        (MpegVersion::V1, MpegLayer::L3) => &MPEG1_L3,
        (_, MpegLayer::L1) => &MPEG2_L1,
        (_, _) => &MPEG2_L23,
    };
    table.get(index - 1).copied()
}

fn mp3_frame_length_bytes(
    version: MpegVersion,
    layer: MpegLayer,
    bitrate_kbps: u32,
    sample_rate: u32,
    padding: u32,
) -> Option<usize> {
    let bitrate = bitrate_kbps.checked_mul(1000)?;
    let value = match layer {
        MpegLayer::L1 => (((12 * bitrate) / sample_rate) + padding) * 4,
        MpegLayer::L2 => ((144 * bitrate) / sample_rate) + padding,
        MpegLayer::L3 => {
            let coefficient = if matches!(version, MpegVersion::V1) {
                144
            } else {
                72
            };
            ((coefficient * bitrate) / sample_rate) + padding
        }
    };
    usize::try_from(value).ok()
}

fn mp3_samples_per_frame(version: MpegVersion, layer: MpegLayer) -> u32 {
    match (version, layer) {
        (_, MpegLayer::L1) => 384,
        (MpegVersion::V1, MpegLayer::L2) => 1152,
        (MpegVersion::V1, MpegLayer::L3) => 1152,
        (_, MpegLayer::L2) => 1152,
        (_, MpegLayer::L3) => 576,
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
