use std::collections::HashMap;

use adk_rust::Content;
use serde_json::{Value, json};
use tracing::info;

/// Log a full LLM request as pretty-printed JSON.
///
/// `phase` is an optional label (e.g. `"synthesis"`) to distinguish
/// planner calls from synthesis calls.
pub fn log_llm_request(
    model: &str,
    contents: &[Content],
    tools: &HashMap<String, Value>,
    phase: Option<&str>,
) {
    let mut entry = json!({
        "type": "llm_request",
        "model": model,
        "message_count": contents.len(),
        "tool_count": tools.len(),
        "tools": tools,
        "contents": contents,
    });
    if let Some(p) = phase {
        entry["phase"] = json!(p);
    }
    info!("\n{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
}

/// Log a full LLM response as pretty-printed JSON.
pub fn log_llm_response(
    model: &str,
    finish_reason: Option<String>,
    content: Option<&Content>,
    phase: Option<&str>,
) {
    let mut entry = json!({
        "type": "llm_response",
        "model": model,
        "finish_reason": finish_reason,
        "content": content,
    });
    if let Some(p) = phase {
        entry["phase"] = json!(p);
    }
    info!("\n{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
}

/// Log a chain step where the engine chose to return a final answer.
pub fn log_chain_step_answer(
    session_id: &str,
    iteration: usize,
    text: &str,
    selection_reason: &str,
) {
    let entry = json!({
        "type": "chain_step",
        "session_id": session_id,
        "iteration": iteration,
        "action": "answer",
        "text": text,
        "selection_reason": selection_reason,
    });
    info!("\n{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
}

/// Log a chain step where the engine chose to ask the user for clarification.
pub fn log_chain_step_ask_user(
    session_id: &str,
    iteration: usize,
    question: &str,
    selection_reason: &str,
) {
    let entry = json!({
        "type": "chain_step",
        "session_id": session_id,
        "iteration": iteration,
        "action": "ask_user",
        "question": question,
        "selection_reason": selection_reason,
    });
    info!("\n{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
}

/// Log a chain step where the engine dispatched a tool call.
pub fn log_chain_step_tool(
    session_id: &str,
    iteration: usize,
    name: &str,
    args: &Value,
    status: &str,
    output: &Value,
    selection_reason: &str,
) {
    let entry = json!({
        "type": "chain_step",
        "session_id": session_id,
        "iteration": iteration,
        "action": "call_tool",
        "tool": name,
        "args": args,
        "status": status,
        "output": output,
        "selection_reason": selection_reason,
    });
    info!("\n{}", serde_json::to_string_pretty(&entry).unwrap_or_default());
}
