use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use adk_rust::{Content, Part};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SessionEntry>>>,
}

#[derive(Debug, Clone, Default)]
struct SessionEntry {
    messages: Vec<Content>,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub session_id: String,
    pub message_count: usize,
    pub updated_at_ms: u64,
    pub last_role: Option<String>,
    pub last_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageView {
    pub role: String,
    pub kind: String,
    pub text: String,
}

impl SessionStore {
    pub async fn append_many<I>(&self, session_id: &str, messages: I)
    where
        I: IntoIterator<Item = Content>,
    {
        let mut guard = self.inner.write().await;
        let entry = guard.entry(session_id.to_string()).or_default();
        entry.messages.extend(messages);
        entry.updated_at_ms = now_ms();
    }

    pub async fn snapshot(&self, session_id: &str) -> Vec<Content> {
        let guard = self.inner.read().await;
        guard
            .get(session_id)
            .map(|entry| entry.messages.clone())
            .unwrap_or_default()
    }

    pub async fn session_message_count(&self, session_id: &str) -> usize {
        let guard = self.inner.read().await;
        guard
            .get(session_id)
            .map(|entry| entry.messages.len())
            .unwrap_or_default()
    }

    pub async fn history(&self, session_id: &str, limit: Option<usize>) -> Vec<MessageView> {
        let guard = self.inner.read().await;
        guard
            .get(session_id)
            .map(|entry| limit_messages(entry.messages.as_slice(), limit))
            .unwrap_or_default()
    }

    pub async fn list(&self) -> Vec<SessionSummary> {
        let guard = self.inner.read().await;
        let mut items = guard
            .iter()
            .map(|(session_id, entry)| SessionSummary {
                session_id: session_id.clone(),
                message_count: entry.messages.len(),
                updated_at_ms: entry.updated_at_ms,
                last_role: entry.messages.last().map(|content| content.role.clone()),
                last_preview: entry.messages.last().map(render_content_preview),
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        items
    }
}

pub fn limit_messages(messages: &[Content], limit: Option<usize>) -> Vec<MessageView> {
    let slice = match limit {
        Some(value) if value < messages.len() => &messages[messages.len() - value..],
        _ => messages,
    };
    slice.iter().map(MessageView::from).collect()
}

impl From<&Content> for MessageView {
    fn from(value: &Content) -> Self {
        let mut saw_text = false;
        let mut saw_tool_call = false;
        let mut saw_tool_result = false;
        let mut lines = Vec::new();

        for part in &value.parts {
            match part {
                Part::Thinking { thinking, .. } => {
                    lines.push(format!("[thinking] {thinking}"));
                }
                Part::Text { text } => {
                    saw_text = true;
                    lines.push(text.clone());
                }
                Part::FunctionCall { name, args, id, .. } => {
                    saw_tool_call = true;
                    let args_text =
                        serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
                    let prefix = id
                        .as_ref()
                        .map(|call_id| format!("{name}#{call_id}"))
                        .unwrap_or_else(|| name.clone());
                    lines.push(format!("{prefix}({args_text})"));
                }
                Part::FunctionResponse {
                    function_response,
                    id,
                } => {
                    saw_tool_result = true;
                    let payload = serde_json::to_string_pretty(&function_response.response)
                        .unwrap_or_else(|_| function_response.response.to_string());
                    let prefix = id
                        .as_ref()
                        .map(|call_id| format!("{}#{call_id}", function_response.name))
                        .unwrap_or_else(|| function_response.name.clone());
                    lines.push(format!("{prefix} => {payload}"));
                }
                Part::InlineData { mime_type, .. } => {
                    lines.push(format!("[inline data: {mime_type}]"));
                }
                Part::FileData {
                    mime_type,
                    file_uri,
                } => {
                    lines.push(format!("[file: {mime_type}] {file_uri}"));
                }
            }
        }

        let kind = match (saw_text, saw_tool_call, saw_tool_result) {
            (true, false, false) => "text",
            (false, true, false) => "tool_call",
            (false, false, true) => "tool_result",
            _ => "mixed",
        };

        Self {
            role: value.role.clone(),
            kind: kind.to_string(),
            text: lines.join("\n"),
        }
    }
}

fn render_content_preview(content: &Content) -> String {
    let rendered = MessageView::from(content).text;
    rendered
        .lines()
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
