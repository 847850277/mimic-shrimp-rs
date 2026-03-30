//! 微信账号长轮询监控。

use std::time::Duration;

use tokio::time::sleep;

use super::{
    service::SESSION_EXPIRED_ERRCODE,
    types::{WeixinAccountRecord, WeixinGetUpdatesResponse},
};
use crate::channel::weixin::service::WeixinManager;

/// 运行单个账号的长轮询循环。
pub async fn run_account_monitor(manager: WeixinManager, account: WeixinAccountRecord) {
    manager
        .mark_runtime_started(account.account_id.as_str())
        .await;

    let mut get_updates_buf = manager
        .load_sync_cursor(account.account_id.as_str())
        .unwrap_or_default()
        .unwrap_or_default();
    let mut next_timeout_ms = manager.config().long_poll_timeout_ms;
    let mut consecutive_failures = 0usize;
    let mut paused_until_ms = None::<u64>;

    loop {
        if let Some(until) = paused_until_ms {
            let now = super::service::now_ms();
            if now < until {
                sleep(Duration::from_millis(until - now)).await;
                continue;
            }
            paused_until_ms = None;
        }

        let response = manager
            .api()
            .get_updates(&account, &get_updates_buf, next_timeout_ms)
            .await;
        match response {
            Ok(payload) => {
                if let Some(timeout_ms) = payload.longpolling_timeout_ms.filter(|value| *value > 0) {
                    next_timeout_ms = timeout_ms;
                }
                if let Some(pause_until) =
                    handle_api_error(&manager, account.account_id.as_str(), &payload).await
                {
                    consecutive_failures = 0;
                    paused_until_ms = Some(pause_until);
                    continue;
                }
                consecutive_failures = 0;
                manager.mark_runtime_event(account.account_id.as_str(), false).await;
                if let Some(cursor) = payload
                    .get_updates_buf
                    .as_deref()
                    .filter(|value| !value.is_empty())
                {
                    if let Err(error) =
                        manager.save_sync_cursor(account.account_id.as_str(), cursor)
                    {
                        manager
                            .mark_runtime_error(
                                account.account_id.as_str(),
                                format!("failed to persist sync cursor: {error}"),
                            )
                            .await;
                    } else {
                        get_updates_buf = cursor.to_string();
                    }
                }
                for message in payload.msgs {
                    manager.mark_runtime_event(account.account_id.as_str(), true).await;
                    if let Err(error) = manager.handle_incoming_message(&account, message).await {
                        manager
                            .mark_runtime_error(account.account_id.as_str(), error.to_string())
                            .await;
                    }
                }
            }
            Err(error) => {
                consecutive_failures += 1;
                manager
                    .mark_runtime_error(account.account_id.as_str(), error.to_string())
                    .await;
                if consecutive_failures >= 3 {
                    consecutive_failures = 0;
                    sleep(Duration::from_millis(manager.config().backoff_delay_ms)).await;
                } else {
                    sleep(Duration::from_millis(manager.config().retry_delay_ms)).await;
                }
            }
        }
    }
}

async fn handle_api_error(
    manager: &WeixinManager,
    account_id: &str,
    payload: &WeixinGetUpdatesResponse,
) -> Option<u64> {
    let ret = payload.ret.unwrap_or_default();
    let errcode = payload.errcode.unwrap_or_default();
    if ret == 0 && errcode == 0 {
        return None;
    }
    let message = payload
        .errmsg
        .clone()
        .unwrap_or_else(|| "unknown weixin getupdates error".to_string());
    if ret == SESSION_EXPIRED_ERRCODE || errcode == SESSION_EXPIRED_ERRCODE {
        let pause_until =
            super::service::now_ms() + manager.config().session_pause_minutes * 60_000;
        manager
            .mark_runtime_error(
                account_id,
                format!(
                    "weixin session expired, pausing until {}",
                    pause_until
                ),
            )
            .await;
        return Some(pause_until);
    }
    manager
        .mark_runtime_error(
            account_id,
            format!("weixin getupdates failed: ret={ret} errcode={errcode} message={message}"),
        )
        .await;
    None
}
