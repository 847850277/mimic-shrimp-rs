//! `capability` 模块负责抽象项目对外提供的核心能力边界。
//! Web 层和 Channel 层只依赖这里定义的能力，而不直接依赖底层引擎实现。

mod conversation;
mod extraction;
mod learning;
mod media_translate;
mod sessions;
mod tools;

use std::sync::Arc;

use crate::{config::MediaTranslateConfig, engine::ToolCallEngine};

pub use conversation::{ConversationCapability, ConversationRequest};
pub use extraction::{StructuredExtractionCapability, StructuredExtractionRequest};
pub use learning::EnglishLearningCapability;
pub use media_translate::{
    MediaTranslateAudioOutput, MediaTranslateCapability, MediaTranslateInput, MediaTranslateRequest,
};
pub use sessions::SessionCapability;
pub use tools::{DirectToolInvocationRequest, ToolCapability};

/// 能力集合入口，聚合当前应用暴露的聊天、会话查询和工具调用能力。
#[derive(Clone)]
pub struct CapabilityHub {
    conversation: ConversationCapability,
    /// 结构化抽取能力，供独立的表单抽取接口调用。
    extraction: StructuredExtractionCapability,
    /// 媒体翻译能力，独立接入阿里百炼媒体翻译接口。
    media_translate: MediaTranslateCapability,
    /// 每日英语学习能力，负责新闻抓取、学习卡片生成和学习口令处理。
    english_learning: EnglishLearningCapability,
    sessions: SessionCapability,
    tools: ToolCapability,
}

impl CapabilityHub {
    /// 基于底层引擎创建完整的能力集合。
    pub fn new(
        engine: Arc<ToolCallEngine>,
        extraction: StructuredExtractionCapability,
        media_translate_config: MediaTranslateConfig,
        english_learning: EnglishLearningCapability,
    ) -> Self {
        Self {
            conversation: ConversationCapability::new(engine.clone()),
            extraction: extraction.clone(),
            media_translate: MediaTranslateCapability::new(media_translate_config),
            english_learning,
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

    /// 返回媒体翻译能力。
    pub fn media_translate(&self) -> &MediaTranslateCapability {
        &self.media_translate
    }

    /// 返回每日英语学习能力。
    pub fn english_learning(&self) -> &EnglishLearningCapability {
        &self.english_learning
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
