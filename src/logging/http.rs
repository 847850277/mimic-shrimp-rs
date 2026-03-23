//! HTTP 日志模块，负责记录 Web 接口层的请求和处理结果。

use serde_json::Value;
use tracing::{error, info};

use super::{preview_json, preview_text};

/// 记录 `/chat` 请求的入口信息。
pub fn log_http_chat_request(
    session_id: &str,
    user_id: &str,
    persist: bool,
    requested_max_iterations: Option<usize>,
    message: &str,
) {
    info!(
        session_id = %session_id,
        user_id = %user_id,
        persist,
        requested_max_iterations = ?requested_max_iterations,
        message_preview = %preview_text(message, 160),
        "received chat request"
    );
}

/// 记录 `/chat` 请求成功完成时的摘要信息。
pub fn log_http_chat_complete(
    session_id: &str,
    user_id: &str,
    iterations: usize,
    tool_call_count: usize,
    finish_reason: Option<&str>,
    answer: &str,
) {
    info!(
        session_id = %session_id,
        user_id = %user_id,
        iterations,
        tool_call_count,
        finish_reason = ?finish_reason,
        answer_preview = %preview_text(answer, 160),
        "completed chat request"
    );
}

/// 记录 `/chat` 请求失败的情况。
pub fn log_http_chat_failed(session_id: &str, user_id: &str, error_message: &str) {
    error!(
        session_id = %session_id,
        user_id = %user_id,
        error = %error_message,
        "chat request failed"
    );
}

/// 记录 `/extract/form` 请求的入口信息。
pub fn log_http_form_extract_request(form_id: Option<&str>, text: &str, inline_schema: bool) {
    info!(
        form_id = ?form_id,
        inline_schema,
        text_preview = %preview_text(text, 160),
        "received form extraction request"
    );
}

/// 记录 `/extract/form` 请求成功完成时的摘要信息。
pub fn log_http_form_extract_complete(
    form_id: Option<&str>,
    missing_field_count: usize,
    invalid_field_count: usize,
    warning_count: usize,
    data: &Value,
) {
    info!(
        form_id = ?form_id,
        missing_field_count,
        invalid_field_count,
        warning_count,
        data_preview = %preview_json(data, 220),
        "completed form extraction request"
    );
}

/// 记录 `/extract/form` 请求失败的情况。
pub fn log_http_form_extract_failed(form_id: Option<&str>, error_message: &str) {
    error!(
        form_id = ?form_id,
        error = %error_message,
        "form extraction request failed"
    );
}

/// 记录直接工具调用接口的入口信息。
pub fn log_http_tool_invoke_request(session_id: &str, user_id: &str, tool: &str, args: &Value) {
    info!(
        session_id = %session_id,
        user_id = %user_id,
        tool = %tool,
        args_preview = %preview_json(args, 200),
        "received direct tool invocation"
    );
}

/// 记录直接工具调用接口成功完成时的结果摘要。
pub fn log_http_tool_invoke_complete(tool: &str, function_call_id: &str, output: &Value) {
    info!(
        tool = %tool,
        function_call_id = %function_call_id,
        output_preview = %preview_json(output, 200),
        "completed direct tool invocation"
    );
}

/// 记录直接工具调用接口失败的情况。
pub fn log_http_tool_invoke_failed(error_message: &str) {
    error!(error = %error_message, "direct tool invocation failed");
}
