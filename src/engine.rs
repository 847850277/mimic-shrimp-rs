use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use adk_rust::{
    Content, FinishReason, FunctionResponseData, Llm, LlmRequest, LlmResponse, Part,
    futures::StreamExt,
};
use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    logging,
    session_store::{MessageView, SessionStore},
    tools::{ToolExecutionFailure, ToolExecutionRequest, ToolExecutionResult, ToolRegistry},
};

const DEFAULT_PLANNER_CANDIDATE_LIMIT: usize = 3;
const DEFAULT_ERROR_BUDGET: usize = 2;
const RECENT_TOOL_SIGNATURE_WINDOW: usize = 4;
const DEFAULT_HISTORY_PROBE_LIMIT: usize = 6;

pub struct ToolCallEngine {
    app_name: String,
    llm: Arc<dyn Llm>,
    registry: ToolRegistry,
    session_store: SessionStore,
    default_system_prompt: String,
    max_iterations: usize,
    max_tool_calls_per_turn: usize,
    planner_candidate_limit: usize,
    error_budget: usize,
}

#[derive(Debug, Clone)]
pub struct ChatTurnRequest {
    pub session_id: String,
    pub user_id: String,
    pub message: String,
    pub system_prompt: Option<String>,
    pub max_iterations: Option<usize>,
    pub persist: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallTrace {
    pub function_call_id: String,
    pub name: String,
    pub args: Value,
    pub status: String,
    pub output: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanningCandidateTrace {
    pub label: String,
    pub action_type: String,
    pub preview: String,
    pub selected: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanningStepTrace {
    pub iteration: usize,
    pub selected_action: String,
    pub selection_reason: String,
    pub observation: Option<String>,
    pub candidates: Vec<PlanningCandidateTrace>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTurnResponse {
    pub session_id: String,
    pub user_id: String,
    pub answer: String,
    pub finish_reason: Option<String>,
    pub iterations: usize,
    pub tool_calls: Vec<ToolCallTrace>,
    pub planning_steps: Vec<PlanningStepTrace>,
    pub turn_messages: Vec<MessageView>,
    pub session_message_count: usize,
}

#[derive(Debug, Clone)]
struct FunctionCallEnvelope {
    function_call_id: String,
    name: String,
    args: Value,
}

#[derive(Debug, Clone)]
enum PlannedAction {
    CallTool(FunctionCallEnvelope),
    Answer { text: String },
    AskUser { question: String },
}

#[derive(Debug, Clone)]
struct ActionCandidate {
    label: String,
    reason: String,
    action: PlannedAction,
}

#[derive(Debug, Clone)]
struct SelectedAction {
    action: PlannedAction,
    selected_preview: String,
    selection_reason: String,
    candidate_traces: Vec<PlanningCandidateTrace>,
}

#[derive(Debug, Default)]
struct TurnState {
    tool_calls_executed: usize,
    tool_errors: usize,
    recent_tool_signatures: VecDeque<String>,
}

impl TurnState {
    fn push_tool_signature(&mut self, signature: String) {
        if self.recent_tool_signatures.len() == RECENT_TOOL_SIGNATURE_WINDOW {
            self.recent_tool_signatures.pop_front();
        }
        self.recent_tool_signatures.push_back(signature);
    }

    fn would_repeat_exact(&self, signature: &str) -> bool {
        self.recent_tool_signatures
            .back()
            .map(|recent| recent == signature)
            .unwrap_or(false)
    }

    fn would_ping_pong(&self, signature: &str) -> bool {
        if self.recent_tool_signatures.len() < 3 {
            return false;
        }

        let len = self.recent_tool_signatures.len();
        let a = &self.recent_tool_signatures[len - 3];
        let b = &self.recent_tool_signatures[len - 2];
        let c = &self.recent_tool_signatures[len - 1];
        a == c && b == signature && a != b
    }
}

impl ToolCallEngine {
    pub fn new(
        app_name: String,
        llm: Arc<dyn Llm>,
        registry: ToolRegistry,
        session_store: SessionStore,
        default_system_prompt: String,
        max_iterations: usize,
    ) -> Self {
        Self {
            app_name,
            llm,
            registry,
            session_store,
            default_system_prompt,
            max_iterations,
            max_tool_calls_per_turn: max_iterations,
            planner_candidate_limit: DEFAULT_PLANNER_CANDIDATE_LIMIT,
            error_budget: DEFAULT_ERROR_BUDGET,
        }
    }

    pub fn tools(&self) -> Vec<crate::tools::ToolDescriptor> {
        self.registry.descriptors()
    }

    pub async fn list_sessions(&self) -> Vec<crate::session_store::SessionSummary> {
        self.session_store.list().await
    }

    pub async fn session_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Vec<MessageView> {
        self.session_store.history(session_id, limit).await
    }

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
                tool_calls_executed = state.tool_calls_executed,
                "planning next action"
            );

            let response = self
                .collect_llm_response(transcript.make_contiguous().to_vec())
                .await?;
            finish_reason = response.finish_reason.as_ref().map(finish_reason_to_string);

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
                PlannedAction::Answer { text } => {
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
                PlannedAction::AskUser { question } => {
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
                PlannedAction::CallTool(function_call) => {
                    let model_call_content = build_model_tool_call_content(&function_call);
                    transcript.push_back(model_call_content.clone());
                    turn_messages.push(model_call_content);

                    let tool_signature =
                        tool_call_signature(&function_call.name, &function_call.args);
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

    async fn collect_llm_response(&self, contents: Vec<Content>) -> Result<LlmResponse> {
        debug!(
            llm = self.llm.name(),
            input_message_count = contents.len(),
            tool_schema_count = self.registry.schemas().len(),
            "collecting planner response stream"
        );
        let mut request = LlmRequest::new(self.llm.name().to_string(), contents);
        request.tools = self.registry.schemas();

        logging::log_llm_request(&request.model, &request.contents, &request.tools, None);

        let mut stream = self.llm.generate_content(request, true).await?;
        let mut all_parts = Vec::new();
        let mut usage_metadata = None;
        let mut finish_reason = None;
        let mut saw_chunk = false;

        while let Some(item) = stream.next().await {
            let response = item?;
            saw_chunk = true;
            if let Some(content) = response.content {
                append_stream_parts(&mut all_parts, content.parts);
            }
            if usage_metadata.is_none() {
                usage_metadata = response.usage_metadata;
            }
            if let Some(reason) = response.finish_reason {
                finish_reason = Some(reason);
            }
        }

        if !saw_chunk {
            return Err(anyhow!("llm returned an empty response stream"));
        }

        let content = if all_parts.is_empty() {
            None
        } else {
            Some(Content {
                role: "model".to_string(),
                parts: all_parts,
            })
        };

        debug!(
            llm = self.llm.name(),
            finish_reason = ?finish_reason.as_ref().map(finish_reason_to_string),
            part_count = content.as_ref().map(|item| item.parts.len()).unwrap_or_default(),
            "assembled planner response stream"
        );

        logging::log_llm_response(
            self.llm.name(),
            finish_reason.as_ref().map(finish_reason_to_string),
            content.as_ref(),
            None,
        );

        Ok(LlmResponse {
            content,
            usage_metadata,
            finish_reason,
            citation_metadata: None,
            partial: false,
            turn_complete: true,
            interrupted: false,
            error_code: None,
            error_message: None,
        })
    }

    fn build_planner_system_prompt(&self, base: &str) -> String {
        format!(
            "{base}\n\nLoop policy:\n- Work as an iterative plan-execute-observe loop.\n- Plan only the next action, never a long chain.\n- Consider up to {} immediate next-step directions before committing one.\n- Commit at most one tool call per iteration.\n- After each tool result, re-plan from the updated transcript.\n- Avoid repeating the same tool with the same arguments.\n- Only return a final answer when you have verified the result is correct and meaningful, not just that a command ran successfully.\n- If a tool result is an error or indicates service unavailability, try a different approach rather than returning that as the answer.\n- Do not spend turns probing whether curl, wget, nc, python, or similar binaries exist unless the user explicitly asked to debug the server environment.\n- If more user input is required, ask one concise clarification question.\n- If the answer is ready, return the final answer directly.",
            self.planner_candidate_limit
        )
    }

    fn plan_candidates(
        &self,
        request: &ChatTurnRequest,
        model_content: &Content,
        has_prior_history: bool,
    ) -> Vec<ActionCandidate> {
        let mut candidates = Vec::new();
        let mut seen_signatures = HashSet::new();

        for function_call in extract_function_calls(model_content) {
            let signature = tool_call_signature(&function_call.name, &function_call.args);
            if !seen_signatures.insert(signature) {
                continue;
            }

            candidates.push(ActionCandidate {
                label: format!("tool:{}", function_call.name),
                reason: "model proposed an immediate tool step".to_string(),
                action: PlannedAction::CallTool(function_call),
            });
            if candidates.len() >= self.planner_candidate_limit {
                return candidates;
            }
        }

        let text = extract_text(model_content);
        if !text.is_empty() && candidates.len() < self.planner_candidate_limit {
            candidates.push(ActionCandidate {
                label: "answer:direct".to_string(),
                reason: "model produced a direct answer candidate".to_string(),
                action: PlannedAction::Answer { text },
            });
        }

        if has_prior_history
            && candidates.len() < self.planner_candidate_limit
            && self.registry.has("sessions_history")
        {
            let args = serde_json::json!({
                "session_id": request.session_id,
                "limit": DEFAULT_HISTORY_PROBE_LIMIT,
            });
            let signature = tool_call_signature("sessions_history", &args);
            if seen_signatures.insert(signature) {
                candidates.push(ActionCandidate {
                    label: "context:history".to_string(),
                    reason:
                        "default context branch that inspects committed session history before another action"
                            .to_string(),
                    action: PlannedAction::CallTool(FunctionCallEnvelope {
                        function_call_id: format!("call-{}", Uuid::new_v4()),
                        name: "sessions_history".to_string(),
                        args,
                    }),
                });
            }
        }

        if candidates.len() < self.planner_candidate_limit {
            candidates.push(ActionCandidate {
                label: "clarify:user".to_string(),
                reason: "default clarification branch when no safer committed step remains"
                    .to_string(),
                action: PlannedAction::AskUser {
                    question:
                        "I need a bit more context to continue safely. What exact result should the next step produce?"
                            .to_string(),
                },
            });
        }

        candidates.truncate(self.planner_candidate_limit);
        candidates
    }

    fn select_action(&self, candidates: Vec<ActionCandidate>, state: &TurnState) -> SelectedAction {
        let mut rejections = vec![String::new(); candidates.len()];
        let mut fallback_ask_user_idx = None;
        let mut selected_idx = None;
        let mut selection_reason = String::new();

        for (index, candidate) in candidates.iter().enumerate() {
            if selected_idx.is_some() {
                break;
            }
            match &candidate.action {
                PlannedAction::CallTool(function_call) => {
                    if state.tool_calls_executed >= self.max_tool_calls_per_turn {
                        rejections[index] = "rejected by max_tool_calls_per_turn guard".to_string();
                        continue;
                    }

                    let signature = tool_call_signature(&function_call.name, &function_call.args);
                    if state.would_repeat_exact(&signature) {
                        rejections[index] =
                            "rejected by repeated-call detection (same tool + same args)"
                                .to_string();
                        continue;
                    }

                    if state.would_ping_pong(&signature) {
                        rejections[index] =
                            "rejected by repeated-call detection (A/B ping-pong)".to_string();
                        continue;
                    }

                    selected_idx = Some(index);
                    selection_reason = format!(
                        "{}; selected the first viable committed tool step",
                        candidate.reason
                    );
                    break;
                }
                PlannedAction::Answer { text } => {
                    if text.trim().is_empty() {
                        rejections[index] = "rejected because the answer text is empty".to_string();
                        continue;
                    }

                    selected_idx = Some(index);
                    selection_reason = format!(
                        "{}; selected the direct answer because no earlier viable tool candidate won",
                        candidate.reason
                    );
                    break;
                }
                PlannedAction::AskUser { question } => {
                    if question.trim().is_empty() {
                        rejections[index] =
                            "rejected because the clarification question is empty".to_string();
                        continue;
                    }
                    fallback_ask_user_idx = Some(index);
                    rejections[index] = "kept as a fallback clarification branch".to_string();
                }
            }
        }

        if selected_idx.is_none() {
            selected_idx = fallback_ask_user_idx;
            if selected_idx.is_some() {
                selection_reason =
                    "selected the clarification branch because no safe tool or final answer candidate remained"
                        .to_string();
            }
        }

        let selected_idx = selected_idx.unwrap_or(0);
        let selected_preview = candidate_preview(&candidates[selected_idx].action);
        let selected_action = candidates[selected_idx].action.clone();
        let candidate_traces = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| PlanningCandidateTrace {
                label: candidate.label.clone(),
                action_type: candidate_action_type(&candidate.action).to_string(),
                preview: candidate_preview(&candidate.action),
                selected: index == selected_idx,
                reason: if index == selected_idx {
                    selection_reason.clone()
                } else if rejections[index].is_empty() {
                    "not selected because an earlier candidate won".to_string()
                } else {
                    rejections[index].clone()
                },
            })
            .collect();

        SelectedAction {
            action: selected_action,
            selected_preview,
            selection_reason,
            candidate_traces,
        }
    }

    fn build_fallback_content(&self, reason: &str, tool_traces: &[ToolCallTrace]) -> Content {
        let summary = if tool_traces.is_empty() {
            format!("I stopped because {reason}. Please clarify the next objective.")
        } else {
            let recent = tool_traces
                .iter()
                .rev()
                .take(2)
                .map(|trace| format!("{} => {}", trace.name, preview_json(&trace.output, 120)))
                .collect::<Vec<_>>()
                .join("; ");
            format!("I stopped because {reason}. Recent tool observations: {recent}")
        };
        Content::new("model").with_text(summary)
    }

    async fn synthesize_final_answer(&self, transcript: &[Content]) -> Option<Content> {
        let mut synthesis_transcript = transcript.to_vec();
        synthesis_transcript.push(Content::new("user").with_text(
            "Based on all the steps and tool results above, provide a clear and concise final answer to the original user request. Use the same language the user used. If you obtained valid data, present it directly. If all attempts failed, explain what went wrong and suggest alternatives. Do not mention iteration limits, internal engine details, or tool names.",
        ));
        let mut request = LlmRequest::new(self.llm.name().to_string(), synthesis_transcript);
        request.tools = std::collections::HashMap::new();

        logging::log_llm_request(&request.model, &request.contents, &request.tools, Some("synthesis"));

        match self.llm.generate_content(request, true).await {
            Ok(mut stream) => {
                let mut parts = Vec::new();
                while let Some(item) = stream.next().await {
                    if let Ok(response) = item {
                        if let Some(content) = response.content {
                            append_stream_parts(&mut parts, content.parts);
                        }
                    }
                }
                let text = parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let text = text.trim().to_string();
                let synthesis_content = if text.is_empty() {
                    None
                } else {
                    Some(Content::new("model").with_text(text))
                };
                logging::log_llm_response(
                    self.llm.name(),
                    None,
                    synthesis_content.as_ref(),
                    Some("synthesis"),
                );
                synthesis_content
            }
            Err(err) => {
                warn!(error = %err, "synthesis LLM call failed, using built-in fallback");
                None
            }
        }
    }

    async fn dispatch_tool_call(
        &self,
        request: &ChatTurnRequest,
        invocation_id: &str,
        function_call_id: &str,
        tool_name: &str,
        args: Value,
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

fn build_model_tool_call_content(function_call: &FunctionCallEnvelope) -> Content {
    Content {
        role: "model".to_string(),
        parts: vec![Part::FunctionCall {
            name: function_call.name.clone(),
            args: function_call.args.clone(),
            id: Some(function_call.function_call_id.clone()),
        }],
    }
}

fn candidate_action_type(action: &PlannedAction) -> &'static str {
    match action {
        PlannedAction::CallTool(_) => "call_tool",
        PlannedAction::Answer { .. } => "answer",
        PlannedAction::AskUser { .. } => "ask_user",
    }
}

fn candidate_preview(action: &PlannedAction) -> String {
    match action {
        PlannedAction::CallTool(function_call) => format!(
            "{}({})",
            function_call.name,
            preview_json(&function_call.args, 120)
        ),
        PlannedAction::Answer { text } => preview_text(text, 160),
        PlannedAction::AskUser { question } => preview_text(question, 160),
    }
}

fn tool_call_signature(name: &str, args: &Value) -> String {
    format!(
        "{}:{}",
        name,
        serde_json::to_string(args).unwrap_or_else(|_| "<invalid-json>".to_string())
    )
}

fn extract_function_calls(content: &Content) -> Vec<FunctionCallEnvelope> {
    content
        .parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| match part {
            Part::FunctionCall { name, args, id } => Some(FunctionCallEnvelope {
                function_call_id: id
                    .clone()
                    .unwrap_or_else(|| format!("call-{}-{index}", Uuid::new_v4())),
                name: name.clone(),
                args: args.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn extract_text(content: &Content) -> String {
    let text = content
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    text.trim().to_string()
}

fn append_stream_parts(target: &mut Vec<Part>, incoming: Vec<Part>) {
    for part in incoming {
        match part {
            Part::Text { text } => {
                if let Some(Part::Text { text: current }) = target.last_mut() {
                    current.push_str(&text);
                } else {
                    target.push(Part::Text { text });
                }
            }
            other => target.push(other),
        }
    }
}

fn finish_reason_to_string(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::MaxTokens => "max_tokens",
        FinishReason::Safety => "safety",
        FinishReason::Recitation => "recitation",
        FinishReason::Other => "other",
    }
    .to_string()
}

fn preview_text(input: &str, limit: usize) -> String {
    let mut preview = input.trim().replace('\n', "\\n");
    if preview.chars().count() > limit {
        preview = preview.chars().take(limit).collect::<String>();
        preview.push_str("...");
    }
    preview
}

fn preview_json(value: &Value, limit: usize) -> String {
    preview_text(
        &serde_json::to_string(value).unwrap_or_else(|_| "<invalid-json>".to_string()),
        limit,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use adk_rust::{Content, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, async_trait};

    use super::*;
    use crate::{
        config::ExecCommandToolConfig, session_store::SessionStore, tools::build_builtin_registry,
    };

    struct ScriptedLlm {
        responses: Mutex<VecDeque<LlmResponse>>,
    }

    impl ScriptedLlm {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
            }
        }
    }

    #[async_trait]
    impl Llm for ScriptedLlm {
        fn name(&self) -> &str {
            "scripted-llm"
        }

        async fn generate_content(
            &self,
            _req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<LlmResponseStream> {
            let next = self
                .responses
                .lock()
                .expect("scripted llm poisoned")
                .pop_front()
                .expect("missing scripted response");
            let stream = adk_rust::futures::stream::once(async move { Ok(next) });
            Ok(Box::pin(stream))
        }
    }

    fn disabled_exec_tool() -> ExecCommandToolConfig {
        ExecCommandToolConfig {
            enabled: false,
            shell: "/bin/sh".to_string(),
            timeout_secs: 20,
            max_output_chars: 4000,
        }
    }

    fn enabled_exec_tool() -> ExecCommandToolConfig {
        ExecCommandToolConfig {
            enabled: true,
            shell: "/bin/sh".to_string(),
            timeout_secs: 20,
            max_output_chars: 4000,
        }
    }

    #[tokio::test]
    async fn executes_iterative_plan_execute_loop_and_persists_turn() {
        let llm = ScriptedLlm::new(vec![
            LlmResponse {
                content: Some(Content {
                    role: "model".to_string(),
                    parts: vec![Part::FunctionCall {
                        name: "math_add".to_string(),
                        args: serde_json::json!({"a": 2.0, "b": 3.0}),
                        id: Some("call_math".to_string()),
                    }],
                }),
                usage_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                citation_metadata: None,
                partial: false,
                turn_complete: true,
                interrupted: false,
                error_code: None,
                error_message: None,
            },
            LlmResponse::new(Content::new("model").with_text("2 + 3 = 5")),
        ]);
        let store = SessionStore::default();
        let registry =
            build_builtin_registry(store.clone(), disabled_exec_tool()).expect("registry");
        let engine = ToolCallEngine::new(
            "test-app".to_string(),
            Arc::new(llm),
            registry,
            store.clone(),
            "use tools".to_string(),
            4,
        );

        let response = engine
            .run_turn(ChatTurnRequest {
                session_id: "main".to_string(),
                user_id: "tester".to_string(),
                message: "what is 2 + 3?".to_string(),
                system_prompt: None,
                max_iterations: None,
                persist: true,
            })
            .await
            .expect("chat response");

        assert_eq!(response.answer, "2 + 3 = 5");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].status, "ok");
        assert_eq!(response.planning_steps.len(), 2);

        let history = store.history("main", None).await;
        assert_eq!(history.len(), 4);
        assert_eq!(history[1].kind, "tool_call");
        assert_eq!(history[2].kind, "tool_result");
    }

    #[tokio::test]
    async fn commits_only_one_tool_per_iteration_even_if_model_emits_multiple_calls() {
        let llm = ScriptedLlm::new(vec![
            LlmResponse {
                content: Some(Content {
                    role: "model".to_string(),
                    parts: vec![
                        Part::FunctionCall {
                            name: "math_add".to_string(),
                            args: serde_json::json!({"a": 1.0, "b": 2.0}),
                            id: Some("call_math".to_string()),
                        },
                        Part::FunctionCall {
                            name: "time_now".to_string(),
                            args: serde_json::json!({}),
                            id: Some("call_time".to_string()),
                        },
                    ],
                }),
                usage_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                citation_metadata: None,
                partial: false,
                turn_complete: true,
                interrupted: false,
                error_code: None,
                error_message: None,
            },
            LlmResponse::new(Content::new("model").with_text("done")),
        ]);
        let store = SessionStore::default();
        let registry =
            build_builtin_registry(store.clone(), disabled_exec_tool()).expect("registry");
        let engine = ToolCallEngine::new(
            "test-app".to_string(),
            Arc::new(llm),
            registry,
            store,
            "use tools".to_string(),
            4,
        );

        let response = engine
            .run_turn(ChatTurnRequest {
                session_id: "main".to_string(),
                user_id: "tester".to_string(),
                message: "do one thing".to_string(),
                system_prompt: None,
                max_iterations: None,
                persist: false,
            })
            .await
            .expect("chat response");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "math_add");
        assert_eq!(response.planning_steps[0].candidates.len(), 3);
        assert!(
            response.planning_steps[0].candidates[1]
                .preview
                .contains("time_now")
        );
    }

    #[tokio::test]
    async fn merges_streamed_text_parts_without_inserting_newlines() {
        let llm = ScriptedLlm::new(vec![LlmResponse {
            content: Some(Content {
                role: "model".to_string(),
                parts: vec![
                    Part::Text {
                        text: "根据".to_string(),
                    },
                    Part::Text {
                        text: "会话历史".to_string(),
                    },
                    Part::Text {
                        text: "，我看到 1 条记录。".to_string(),
                    },
                ],
            }),
            usage_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            citation_metadata: None,
            partial: false,
            turn_complete: true,
            interrupted: false,
            error_code: None,
            error_message: None,
        }]);
        let store = SessionStore::default();
        let registry =
            build_builtin_registry(store.clone(), disabled_exec_tool()).expect("registry");
        let engine = ToolCallEngine::new(
            "test-app".to_string(),
            Arc::new(llm),
            registry,
            store,
            "use tools".to_string(),
            4,
        );

        let response = engine
            .run_turn(ChatTurnRequest {
                session_id: "main".to_string(),
                user_id: "tester".to_string(),
                message: "show history".to_string(),
                system_prompt: None,
                max_iterations: None,
                persist: false,
            })
            .await
            .expect("chat response");

        assert_eq!(response.answer, "根据会话历史，我看到 1 条记录。");
    }

    #[tokio::test]
    async fn converges_after_successful_exec_command_results() {
        // The LLM calls exec_command once, gets weather data, then returns a direct answer.
        let llm = ScriptedLlm::new(vec![
            LlmResponse {
                content: Some(Content {
                    role: "model".to_string(),
                    parts: vec![Part::FunctionCall {
                        name: "exec_command".to_string(),
                        args: serde_json::json!({"cmd": "printf '{\"weather\":\"晴\",\"temp\":25}'"}),
                        id: Some("call_exec_1".to_string()),
                    }],
                }),
                usage_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                citation_metadata: None,
                partial: false,
                turn_complete: true,
                interrupted: false,
                error_code: None,
                error_message: None,
            },
            LlmResponse {
                content: Some(Content {
                    role: "model".to_string(),
                    parts: vec![Part::Text {
                        text: "北京今天天气晴，气温25度。".to_string(),
                    }],
                }),
                usage_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                citation_metadata: None,
                partial: false,
                turn_complete: true,
                interrupted: false,
                error_code: None,
                error_message: None,
            },
        ]);
        let store = SessionStore::default();
        let registry =
            build_builtin_registry(store.clone(), enabled_exec_tool()).expect("registry");
        let engine = ToolCallEngine::new(
            "test-app".to_string(),
            Arc::new(llm),
            registry,
            store,
            "use tools".to_string(),
            12,
        );

        let response = engine
            .run_turn(ChatTurnRequest {
                session_id: "main".to_string(),
                user_id: "tester".to_string(),
                message: "weather".to_string(),
                system_prompt: None,
                max_iterations: None,
                persist: false,
            })
            .await
            .expect("chat response");

        assert_eq!(response.tool_calls.len(), 1);
        assert!(
            response.answer.contains("晴"),
            "answer should contain weather result: {}",
            response.answer
        );
    }

    #[test]
    fn detects_a_b_ping_pong_repeats() {
        let mut state = TurnState::default();
        state.push_tool_signature("tool:A".to_string());
        state.push_tool_signature("tool:B".to_string());
        state.push_tool_signature("tool:A".to_string());

        assert!(state.would_ping_pong("tool:B"));
        assert!(!state.would_ping_pong("tool:C"));
    }
}
