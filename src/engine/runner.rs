//! 执行子模块负责驱动完整回合：组装 transcript、逐轮规划、执行工具、提交结果并落会话。

use std::collections::VecDeque;

use adk_rust::{Content, FunctionResponseData, Part};
use anyhow::{Result, anyhow};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{
    ChatTurnRequest, ChatTurnResponse, PlanningStepTrace, ToolCallEngine, ToolCallTrace, TurnState,
    build_model_tool_call_content, extract_text, preview_json, preview_text,
};
use crate::{
    logging,
    session_store::MessageView,
    tools::{ToolExecutionFailure, ToolExecutionRequest, ToolExecutionResult},
};

impl ToolCallEngine {
    /// 执行一次完整的 plan-execute-observe 回合。
    /// 该函数会在单轮内循环规划和执行，直到得到最终答案、需要追问，或触发兜底逻辑。
    pub async fn run_turn(&self, request: ChatTurnRequest) -> Result<ChatTurnResponse> {
        let base_system_prompt = request
            .system_prompt
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.default_system_prompt.clone());
        let system_prompt = self.build_planner_system_prompt(&base_system_prompt);
        let max_iterations = request.max_iterations.unwrap_or(self.max_iterations);
        let user_content = Content::new("user").with_text(request.message.clone());
        let prior_messages = self.session_store.snapshot(&request.session_id).await;
        let prior_message_count = prior_messages.len();

        let mut transcript = VecDeque::new();
        transcript.push_back(Content::new("system").with_text(system_prompt.clone()));
        transcript.extend(prior_messages);
        transcript.push_back(user_content.clone());

        let invocation_id = Uuid::new_v4().to_string();
        let mut turn_messages = vec![user_content];
        let mut tool_traces = Vec::new();
        let mut planning_steps = Vec::new();
        let mut state = TurnState::default();
        let mut iterations = 0usize;
        let mut final_content = None;
        let mut finish_reason = None;

        info!(
            session_id = %request.session_id,
            user_id = %request.user_id,
            max_iterations,
            max_tool_calls_per_turn = self.max_tool_calls_per_turn,
            error_budget = self.error_budget,
            persisted_history = request.persist,
            prior_message_count,
            "starting iterative plan-execute turn"
        );
        debug!(
            session_id = %request.session_id,
            system_prompt_preview = %preview_text(&system_prompt, 200),
            user_message_preview = %preview_text(&request.message, 200),
            "prepared iterative plan transcript"
        );

        for index in 0..max_iterations {
            iterations = index + 1;
            info!(
                session_id = %request.session_id,
                user_id = %request.user_id,
                iteration = iterations,
                transcript_messages = transcript.len(),
                llm_context_message_limit = self.max_context_messages,
                tool_calls_executed = state.tool_calls_executed,
                "planning next action"
            );

            let llm_contents =
                super::build_llm_context_window(&transcript, self.max_context_messages);
            let response = self.collect_llm_response(llm_contents).await?;
            finish_reason = response
                .finish_reason
                .as_ref()
                .map(super::finish_reason_to_string);

            let model_content = response
                .content
                .unwrap_or_else(|| Content::new("model").with_text(""));
            let candidates =
                self.plan_candidates(&request, &model_content, prior_message_count > 0);
            let selection = self.select_action(candidates, &state);

            info!(
                session_id = %request.session_id,
                iteration = iterations,
                selected_action = %selection.selected_preview,
                selection_reason = %selection.selection_reason,
                "selected next committed action"
            );

            match selection.action {
                super::PlannedAction::Answer { text } => {
                    let content = Content::new("model").with_text(text.clone());
                    transcript.push_back(content.clone());
                    turn_messages.push(content.clone());
                    planning_steps.push(PlanningStepTrace {
                        iteration: iterations,
                        selected_action: selection.selected_preview.clone(),
                        selection_reason: selection.selection_reason.clone(),
                        observation: Some("returned final answer".to_string()),
                        candidates: selection.candidate_traces,
                    });
                    logging::log_chain_step_answer(
                        &request.session_id,
                        iterations,
                        &text,
                        &selection.selection_reason,
                    );
                    final_content = Some(content);
                    break;
                }
                super::PlannedAction::AskUser { question } => {
                    let content = Content::new("model").with_text(question.clone());
                    transcript.push_back(content.clone());
                    turn_messages.push(content.clone());
                    planning_steps.push(PlanningStepTrace {
                        iteration: iterations,
                        selected_action: selection.selected_preview.clone(),
                        selection_reason: selection.selection_reason.clone(),
                        observation: Some("asking user for clarification".to_string()),
                        candidates: selection.candidate_traces,
                    });
                    logging::log_chain_step_ask_user(
                        &request.session_id,
                        iterations,
                        &question,
                        &selection.selection_reason,
                    );
                    finish_reason = Some("need_user".to_string());
                    final_content = Some(content);
                    break;
                }
                super::PlannedAction::CallTool(function_call) => {
                    let model_call_content = build_model_tool_call_content(&function_call);
                    transcript.push_back(model_call_content.clone());
                    turn_messages.push(model_call_content);

                    let tool_signature =
                        super::tool_call_signature(&function_call.name, &function_call.args);
                    let tool_result = self
                        .dispatch_tool_call(
                            &request,
                            &invocation_id,
                            &function_call.function_call_id,
                            &function_call.name,
                            function_call.args.clone(),
                        )
                        .await;

                    state.tool_calls_executed += 1;
                    state.push_tool_signature(tool_signature);

                    let (tool_trace, tool_content, observation) = match tool_result {
                        Ok(result) => {
                            info!(
                                session_id = %request.session_id,
                                iteration = iterations,
                                function_call_id = %result.function_call_id,
                                tool = %result.tool_name,
                                output_preview = %preview_json(&result.output, 200),
                                "tool call completed"
                            );
                            let output_preview = preview_json(&result.output, 160);
                            (
                                ToolCallTrace {
                                    function_call_id: result.function_call_id.clone(),
                                    name: result.tool_name.clone(),
                                    args: result.args.clone(),
                                    status: "ok".to_string(),
                                    output: result.output.clone(),
                                },
                                Content {
                                    role: "tool".to_string(),
                                    parts: vec![Part::FunctionResponse {
                                        function_response: FunctionResponseData {
                                            name: result.tool_name,
                                            response: result.output,
                                        },
                                        id: Some(result.function_call_id),
                                    }],
                                },
                                format!("tool completed successfully: {output_preview}"),
                            )
                        }
                        Err(error) => {
                            state.tool_errors += 1;
                            warn!(
                                session_id = %request.session_id,
                                iteration = iterations,
                                function_call_id = %function_call.function_call_id,
                                tool = %function_call.name,
                                error = %error,
                                "tool call failed"
                            );
                            let failure = ToolExecutionFailure {
                                function_call_id: function_call.function_call_id.clone(),
                                tool_name: function_call.name.clone(),
                                args: function_call.args.clone(),
                                message: error.to_string(),
                            };
                            let payload = serde_json::json!({
                                "status": "error",
                                "message": failure.message,
                            });
                            (
                                ToolCallTrace {
                                    function_call_id: failure.function_call_id.clone(),
                                    name: failure.tool_name.clone(),
                                    args: failure.args.clone(),
                                    status: "error".to_string(),
                                    output: payload.clone(),
                                },
                                Content {
                                    role: "tool".to_string(),
                                    parts: vec![Part::FunctionResponse {
                                        function_response: FunctionResponseData {
                                            name: failure.tool_name,
                                            response: payload.clone(),
                                        },
                                        id: Some(failure.function_call_id),
                                    }],
                                },
                                format!("tool failed: {}", preview_json(&payload, 160)),
                            )
                        }
                    };

                    transcript.push_back(tool_content.clone());
                    turn_messages.push(tool_content);

                    logging::log_chain_step_tool(
                        &request.session_id,
                        iterations,
                        &tool_trace.name,
                        &tool_trace.args,
                        &tool_trace.status,
                        &tool_trace.output,
                        &selection.selection_reason,
                    );

                    tool_traces.push(tool_trace);
                    planning_steps.push(PlanningStepTrace {
                        iteration: iterations,
                        selected_action: selection.selected_preview,
                        selection_reason: selection.selection_reason,
                        observation: Some(observation),
                        candidates: selection.candidate_traces,
                    });

                    if state.tool_errors >= self.error_budget {
                        let content = self.build_fallback_content(
                            "error budget exhausted before the turn converged",
                            &tool_traces,
                        );
                        transcript.push_back(content.clone());
                        turn_messages.push(content.clone());
                        final_content = Some(content);
                        finish_reason = Some("error_budget".to_string());
                        break;
                    }
                }
            }
        }

        if final_content.is_none() {
            let content = self
                .synthesize_final_answer(transcript.make_contiguous())
                .await
                .unwrap_or_else(|| {
                    self.build_fallback_content(
                        "max_iterations reached before selecting a final answer",
                        &tool_traces,
                    )
                });
            transcript.push_back(content.clone());
            turn_messages.push(content.clone());
            final_content = Some(content);
            finish_reason = Some("max_iterations".to_string());
        }

        let final_content = final_content.ok_or_else(|| {
            anyhow!("iterative plan-execute loop ended without a terminal response")
        })?;

        if request.persist {
            self.session_store
                .append_many(&request.session_id, turn_messages.iter().cloned())
                .await;
            debug!(
                session_id = %request.session_id,
                appended_messages = turn_messages.len(),
                "persisted committed turn messages to session store"
            );
        }

        let answer = extract_text(&final_content);
        let session_message_count = if request.persist {
            self.session_store
                .session_message_count(&request.session_id)
                .await
        } else {
            turn_messages.len()
        };

        info!(
            session_id = %request.session_id,
            user_id = %request.user_id,
            iterations,
            tool_call_count = tool_traces.len(),
            answer_preview = %preview_text(&answer, 200),
            session_message_count,
            "finished iterative plan-execute turn"
        );

        Ok(ChatTurnResponse {
            session_id: request.session_id,
            user_id: request.user_id,
            answer,
            finish_reason,
            iterations,
            tool_calls: tool_traces,
            planning_steps,
            turn_messages: turn_messages.iter().map(MessageView::from).collect(),
            session_message_count,
        })
    }

    /// 执行一次工具调用，并把请求参数整理成统一的工具执行上下文。
    pub(crate) async fn dispatch_tool_call(
        &self,
        request: &ChatTurnRequest,
        invocation_id: &str,
        function_call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolExecutionResult> {
        self.registry
            .execute(ToolExecutionRequest {
                app_name: self.app_name.clone(),
                user_id: request.user_id.clone(),
                session_id: request.session_id.clone(),
                invocation_id: invocation_id.to_string(),
                function_call_id: function_call_id.to_string(),
                tool_name: tool_name.to_string(),
                args,
                user_content: Content::new("user").with_text(request.message.clone()),
            })
            .await
    }
}
