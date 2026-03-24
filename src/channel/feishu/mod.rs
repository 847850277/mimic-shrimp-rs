//! 飞书通道模块。
//! 这里按“回调解析 / IM 消息处理 / 服务编排”三个子模块组织代码。

mod callback;
mod im;
mod service;

pub use callback::{FeishuCallbackErrorKind, extract_event_type, process_callback};
pub use im::{FeishuBotClient, estimate_audio_duration_ms, parse_message_event};
pub use service::{callback_ack, handle_audio_message_event, handle_text_message_event};
