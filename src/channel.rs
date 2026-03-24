//! `channel` 模块负责承载外部消息通道接入层。
//! 当前实现只有飞书，后续可以在这里继续接入钉钉、企业微信等通道。

pub mod feishu;
#[path = "channel/types.rs"]
mod types;

pub use types::{
    ChannelKind, InboundAudioMessage, InboundMessageParseOutcome, InboundTextMessage,
    OutboundAudioReply, OutboundTextReply,
};
