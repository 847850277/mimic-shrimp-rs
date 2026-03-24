use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use adk_rust::{
    Content, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, async_trait,
};

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
                    thought_signature: None,
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
        build_builtin_registry(store.clone(), disabled_exec_tool(), None).expect("registry");
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
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        name: "time_now".to_string(),
                        args: serde_json::json!({}),
                        id: Some("call_time".to_string()),
                        thought_signature: None,
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
        build_builtin_registry(store.clone(), disabled_exec_tool(), None).expect("registry");
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
        build_builtin_registry(store.clone(), disabled_exec_tool(), None).expect("registry");
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
    let llm = ScriptedLlm::new(vec![
        LlmResponse {
            content: Some(Content {
                role: "model".to_string(),
                parts: vec![Part::FunctionCall {
                    name: "exec_command".to_string(),
                    args: serde_json::json!({"cmd": "printf '{\"weather\":\"晴\",\"temp\":25}'"}),
                    id: Some("call_exec_1".to_string()),
                    thought_signature: None,
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
        build_builtin_registry(store.clone(), enabled_exec_tool(), None).expect("registry");
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
