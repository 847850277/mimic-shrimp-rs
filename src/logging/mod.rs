//! `logging` 模块负责统一管理项目中的结构化日志输出。
//! 外部模块只依赖这里导出的函数，不直接关心底层具体分类文件。

mod channel;
mod engine;
mod http;
mod learning;
mod llm;
mod preview;
mod startup;

pub use channel::{
    log_channel_audio_message_received, log_channel_audio_transcribed,
    log_channel_background_error, log_channel_callback_body_read_error,
    log_channel_callback_ingress, log_channel_callback_json_error,
    log_channel_callback_process_error, log_channel_callback_processed,
    log_channel_message_ignored, log_channel_message_parse_error, log_channel_reply_stage,
    log_channel_reply_success, log_channel_resource_fetch_retry, log_channel_text_message_received,
    log_channel_text_replied,
};
pub use engine::{log_chain_step_answer, log_chain_step_ask_user, log_chain_step_tool};
pub use http::{
    log_http_chat_complete, log_http_chat_failed, log_http_chat_request,
    log_http_form_extract_complete, log_http_form_extract_failed, log_http_form_extract_request,
    log_http_media_translate_complete, log_http_media_translate_failed,
    log_http_media_translate_request, log_http_tool_invoke_complete, log_http_tool_invoke_failed,
    log_http_tool_invoke_request,
};
pub use learning::{
    log_learning_background_error, log_learning_command_handled, log_learning_config,
    log_learning_lesson_saved, log_learning_news_ingest_started, log_learning_news_selected,
    log_learning_scheduler_started,
};
pub use llm::{log_llm_request, log_llm_response};
pub use preview::{preview_bytes, preview_json, preview_text};
pub use startup::{log_feishu_integration_config, log_feishu_reply_disabled, log_service_startup};
