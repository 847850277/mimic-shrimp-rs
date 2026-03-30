//! 通道公共类型模块，负责定义多通道共享的统一入站消息模型。

/// 当前系统支持或计划支持的通道类型。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Feishu,
    Dingtalk,
    Wecom,
    Weixin,
}

impl ChannelKind {
    /// 返回通道类型对应的稳定字符串标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::Dingtalk => "dingtalk",
            Self::Wecom => "wecom",
            Self::Weixin => "weixin",
        }
    }
}

/// 统一的入站文本消息模型。
/// 不同通道在解析后都应尽量收敛到这套字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundTextMessage {
    pub channel: ChannelKind,
    pub event_id: Option<String>,
    pub message_id: String,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub user_id: String,
    pub session_id: String,
    pub text: String,
}

/// 统一的入站音频消息模型。
/// 当前先覆盖语音消息需要的最小字段，后续如果接视频/文件可再继续扩展。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAudioMessage {
    pub channel: ChannelKind,
    pub event_id: Option<String>,
    pub message_id: String,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub user_id: String,
    pub session_id: String,
    pub file_key: String,
    pub resource_type: String,
    pub format_hint: Option<String>,
    pub duration_ms: Option<u64>,
}

/// 通用消息解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundMessageParseOutcome {
    NotMessageEvent,
    Ignored { reason: &'static str },
    Text(InboundTextMessage),
    Audio(InboundAudioMessage),
}

/// 统一的出站文本回复模型。
/// 不同通道在发送回复前都应尽量收敛到这套字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundTextReply {
    pub channel: ChannelKind,
    pub reply_to_message_id: String,
    pub session_id: String,
    pub text: String,
}

/// 统一的出站音频回复模型。
/// 当前先覆盖飞书音频回发所需的最小字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundAudioReply {
    pub channel: ChannelKind,
    pub reply_to_message_id: String,
    pub session_id: String,
    pub file_name: String,
    pub file_format: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub duration_ms: Option<u64>,
}
