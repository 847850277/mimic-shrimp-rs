//! 微信通道本地状态存储。

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::sync::RwLock;

use super::types::{WeixinAccountRecord, WeixinAccountRuntimeState, WeixinAccountSummary};

/// 账号持久化存储。
#[derive(Clone)]
pub struct WeixinStore {
    state_dir: PathBuf,
    runtime: Arc<RwLock<HashMap<String, WeixinAccountRuntimeState>>>,
}

impl WeixinStore {
    /// 创建一个新的微信状态存储。
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            runtime: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 返回全部账号摘要。
    pub async fn list_accounts(&self) -> Result<Vec<WeixinAccountSummary>> {
        let runtime = self.runtime.read().await;
        let mut items = self
            .load_accounts()?
            .into_iter()
            .map(|account| {
                let state = runtime.get(account.account_id.as_str()).cloned().unwrap_or_default();
                WeixinAccountSummary {
                    account_id: account.account_id,
                    linked_user_id: account.linked_user_id,
                    configured: !account.bot_token.trim().is_empty(),
                    running: state.running,
                    saved_at_ms: account.saved_at_ms,
                    last_start_at_ms: state.last_start_at_ms,
                    last_event_at_ms: state.last_event_at_ms,
                    last_inbound_at_ms: state.last_inbound_at_ms,
                    last_error: state.last_error,
                }
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| right.saved_at_ms.cmp(&left.saved_at_ms));
        Ok(items)
    }

    /// 读取全部账号。
    pub fn load_accounts(&self) -> Result<Vec<WeixinAccountRecord>> {
        let dir = self.accounts_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&path)?;
            let account = serde_json::from_str::<WeixinAccountRecord>(&raw)?;
            items.push(account);
        }
        Ok(items)
    }

    /// 按账号 ID 读取账号。
    pub fn load_account(&self, account_id: &str) -> Result<Option<WeixinAccountRecord>> {
        let path = self.account_path(account_id);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    /// 保存账号。
    pub fn save_account(&self, account: &WeixinAccountRecord) -> Result<()> {
        fs::create_dir_all(self.accounts_dir())?;
        fs::write(self.account_path(&account.account_id), serde_json::to_vec_pretty(account)?)?;
        Ok(())
    }

    /// 读取同步游标。
    pub fn load_sync_cursor(&self, account_id: &str) -> Result<Option<String>> {
        let path = self.sync_path(account_id);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path)?;
        let payload = serde_json::from_str::<HashMap<String, String>>(&raw)?;
        Ok(payload.get("get_updates_buf").cloned())
    }

    /// 保存同步游标。
    pub fn save_sync_cursor(&self, account_id: &str, cursor: &str) -> Result<()> {
        fs::create_dir_all(self.sync_dir())?;
        let mut payload = HashMap::new();
        payload.insert("get_updates_buf".to_string(), cursor.to_string());
        payload.insert("updated_at_ms".to_string(), now_ms().to_string());
        fs::write(self.sync_path(account_id), serde_json::to_vec_pretty(&payload)?)?;
        Ok(())
    }

    /// 读取指定账号的 context token 映射。
    pub fn load_context_tokens(&self, account_id: &str) -> Result<HashMap<String, String>> {
        let path = self.context_path(account_id);
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// 保存指定账号的 context token 映射。
    pub fn save_context_tokens(
        &self,
        account_id: &str,
        tokens: &HashMap<String, String>,
    ) -> Result<()> {
        fs::create_dir_all(self.context_dir())?;
        fs::write(self.context_path(account_id), serde_json::to_vec(tokens)?)?;
        Ok(())
    }

    /// 更新账号运行时状态。
    pub async fn mutate_runtime<F>(&self, account_id: &str, mutate: F)
    where
        F: FnOnce(&mut WeixinAccountRuntimeState),
    {
        let mut runtime = self.runtime.write().await;
        let entry = runtime.entry(account_id.to_string()).or_default();
        mutate(entry);
    }

    fn accounts_dir(&self) -> PathBuf {
        self.state_dir.join("accounts")
    }

    fn sync_dir(&self) -> PathBuf {
        self.state_dir.join("sync")
    }

    fn context_dir(&self) -> PathBuf {
        self.state_dir.join("context")
    }

    fn account_path(&self, account_id: &str) -> PathBuf {
        self.accounts_dir()
            .join(format!("{}.json", encode_component(account_id)))
    }

    fn sync_path(&self, account_id: &str) -> PathBuf {
        self.sync_dir()
            .join(format!("{}.json", encode_component(account_id)))
    }

    fn context_path(&self, account_id: &str) -> PathBuf {
        self.context_dir()
            .join(format!("{}.json", encode_component(account_id)))
    }
}

fn encode_component(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(value.as_bytes())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(dead_code)]
fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
