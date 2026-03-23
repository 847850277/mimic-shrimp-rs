//! `capability` 模块负责抽象项目对外提供的核心能力边界。
//! Web 层和 Channel 层只依赖这里定义的能力，而不直接依赖底层引擎实现。

mod conversation;
mod extraction;
mod sessions;
mod tools;

use std::sync::Arc;

use adk_rust::Llm;

use crate::engine::ToolCallEngine;

pub use conversation::{ConversationCapability, ConversationRequest};
pub use extraction::{StructuredExtractionCapability, StructuredExtractionRequest};
pub use sessions::SessionCapability;
pub use tools::{DirectToolInvocationRequest, ToolCapability};

/// 能力集合入口，聚合当前应用暴露的聊天、会话查询和工具调用能力。
#[derive(Clone)]
pub struct CapabilityHub {
    conversation: ConversationCapability,
    /// 结构化抽取能力，供独立的表单抽取接口调用。
    extraction: StructuredExtractionCapability,
    sessions: SessionCapability,
    tools: ToolCapability,
}

impl CapabilityHub {
    /// 基于底层引擎创建完整的能力集合。
    pub fn new(engine: Arc<ToolCallEngine>, llm: Arc<dyn Llm>) -> Self {
        Self {
            conversation: ConversationCapability::new(engine.clone()),
            extraction: StructuredExtractionCapability::new(llm),
            sessions: SessionCapability::new(engine.clone()),
            tools: ToolCapability::new(engine),
        }
    }

    /// 返回聊天回合能力。
    pub fn conversation(&self) -> &ConversationCapability {
        &self.conversation
    }

    /// 返回结构化抽取能力。
    pub fn extraction(&self) -> &StructuredExtractionCapability {
        &self.extraction
    }

    /// 返回会话查询能力。
    pub fn sessions(&self) -> &SessionCapability {
        &self.sessions
    }

    /// 返回工具目录与直接调用能力。
    pub fn tools(&self) -> &ToolCapability {
        &self.tools
    }
}
