//! `engine` 模块负责维护一次完整的 plan-execute-observe 推理回合。
//! 这里保留对外可见的引擎入口，并将规划、执行、状态、LLM 交互等细节拆到子模块中。

use std::sync::Arc;

use adk_rust::{Content, Llm};
use anyhow::Result;
use serde_json::Value;
use tracing::info;
use uuid::Uuid;

use crate::{
    session_store::{MessageView, SessionStore},
    tools::{ToolExecutionRequest, ToolExecutionResult, ToolRegistry},
};

#[path = "engine/llm.rs"]
mod llm;
#[path = "engine/planner.rs"]
mod planner;
#[path = "engine/runner.rs"]
mod runner;
#[path = "engine/state.rs"]
mod state;
#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
#[path = "engine/types.rs"]
mod types;
#[path = "engine/util.rs"]
mod util;

pub use types::{
    ChatTurnRequest, ChatTurnResponse, PlanningCandidateTrace, PlanningStepTrace, ToolCallTrace,
};

pub(crate) use state::TurnState;
pub(crate) use types::{ActionCandidate, FunctionCallEnvelope, PlannedAction, SelectedAction};
pub(crate) use util::{
    append_stream_parts, build_llm_context_window, build_model_tool_call_content,
    candidate_action_type, candidate_preview, extract_function_calls, extract_text,
    finish_reason_to_string, preview_json, preview_text, tool_call_signature,
};

pub(crate) const DEFAULT_PLANNER_CANDIDATE_LIMIT: usize = 3;
pub(crate) const DEFAULT_ERROR_BUDGET: usize = 2;
pub(crate) const RECENT_TOOL_SIGNATURE_WINDOW: usize = 4;
pub(crate) const DEFAULT_HISTORY_PROBE_LIMIT: usize = 6;

/// `ToolCallEngine` 是工具调用引擎的统一入口。
/// 它负责协调 LLM、工具注册表、会话存储以及一次回合内的执行约束。
pub struct ToolCallEngine {
    app_name: String,
    llm: Arc<dyn Llm>,
    registry: ToolRegistry,
    session_store: SessionStore,
    default_system_prompt: String,
    max_iterations: usize,
    max_context_messages: usize,
    max_tool_calls_per_turn: usize,
    planner_candidate_limit: usize,
    error_budget: usize,
}

impl ToolCallEngine {
    /// 创建一个新的工具调用引擎实例，并初始化默认的回合控制参数。
    pub fn new(
        app_name: String,
        llm: Arc<dyn Llm>,
        registry: ToolRegistry,
        session_store: SessionStore,
        default_system_prompt: String,
        max_iterations: usize,
        max_context_messages: usize,
    ) -> Self {
        Self {
            app_name,
            llm,
            registry,
            session_store,
            default_system_prompt,
            max_iterations,
            max_context_messages,
            max_tool_calls_per_turn: max_iterations,
            planner_candidate_limit: DEFAULT_PLANNER_CANDIDATE_LIMIT,
            error_budget: DEFAULT_ERROR_BUDGET,
        }
    }

    /// 返回当前引擎注册的全部工具描述，供接口或调试页面展示。
    pub fn tools(&self) -> Vec<crate::tools::ToolDescriptor> {
        self.registry.descriptors()
    }

    /// 列出当前会话仓库中的全部会话摘要。
    pub async fn list_sessions(&self) -> Vec<crate::session_store::SessionSummary> {
        self.session_store.list().await
    }

    /// 读取指定会话的历史消息，用于调试或显式历史查看。
    pub async fn session_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Vec<MessageView> {
        self.session_store.history(session_id, limit).await
    }

    /// 直接调用指定工具。
    /// 这个入口绕过 plan-execute loop，通常由调试接口单独使用。
    pub async fn invoke_tool(
        &self,
        user_id: String,
        session_id: String,
        tool_name: String,
        args: Value,
    ) -> Result<ToolExecutionResult> {
        info!(
            session_id = %session_id,
            user_id = %user_id,
            tool = %tool_name,
            args_preview = %preview_json(&args, 200),
            "dispatching direct tool invocation"
        );
        self.registry
            .execute(ToolExecutionRequest {
                app_name: self.app_name.clone(),
                user_id,
                session_id,
                invocation_id: Uuid::new_v4().to_string(),
                function_call_id: Uuid::new_v4().to_string(),
                tool_name,
                args,
                user_content: Content::new("user").with_text("direct tool invocation"),
            })
            .await
    }
}
