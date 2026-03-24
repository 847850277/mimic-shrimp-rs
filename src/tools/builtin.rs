//! 内置工具模块，负责构建当前项目自带的工具集合。

use std::sync::Arc;

use adk_rust::{
    AdkError, Tool,
    serde_json::{self},
    tool::FunctionTool,
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    capability::EnglishLearningCapability,
    config::ExecCommandToolConfig,
    session_store::{MessageView, SessionStore, SessionSummary},
};

use super::{
    ToolRegistry,
    exec::{ExecCommandArgs, ExecCommandResult, format_unix_ms, run_exec_command},
};

/// `sessions_list` 工具的输入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SessionsListArgs {
    #[serde(default)]
    limit: Option<usize>,
}

/// `sessions_history` 工具的输入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SessionsHistoryArgs {
    session_id: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// `sessions_history` 工具的输出结构。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct SessionsHistoryResult {
    session_id: String,
    messages: Vec<MessageView>,
}

/// `math_add` 工具的输入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct MathAddArgs {
    a: f64,
    b: f64,
}

/// `math_add` 工具的输出结构。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct MathAddResult {
    sum: f64,
}

/// `time_now` 工具的输出结构。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct TimeNowResult {
    unix_ms: u64,
    utc_hint: String,
}

/// 英语学习工具的统一文本输出结构。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct EnglishLearningReplyResult {
    action: String,
    reply: String,
}

/// 跟读反馈工具的输入参数。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct EnglishLearningShadowingArgs {
    transcript: String,
}

/// 跟读反馈工具的输出结构。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct EnglishLearningShadowingResult {
    handled: bool,
    reply: String,
}

/// 无参数工具的空输入结构。
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
struct EmptyArgs {}

/// 构建当前项目内置的全部工具注册表。
pub fn build_builtin_registry(
    session_store: SessionStore,
    exec_command_config: ExecCommandToolConfig,
    english_learning: Option<EnglishLearningCapability>,
) -> Result<ToolRegistry> {
    let list_store = session_store.clone();
    let history_store = session_store.clone();

    let sessions_list = Arc::new(
        FunctionTool::new(
            "sessions_list",
            "List in-memory sessions and their latest message preview.",
            move |_ctx, args| {
                let store = list_store.clone();
                async move {
                    let input: SessionsListArgs = serde_json::from_value(args)?;
                    let mut items = store.list().await;
                    if let Some(limit) = input.limit {
                        items.truncate(limit);
                    }
                    Ok(serde_json::to_value(items)?)
                }
            },
        )
        .with_parameters_schema::<SessionsListArgs>()
        .with_response_schema::<Vec<SessionSummary>>(),
    ) as Arc<dyn Tool>;

    let sessions_history = Arc::new(
        FunctionTool::new(
            "sessions_history",
            "Read recent message history from one session.",
            move |_ctx, args| {
                let store = history_store.clone();
                async move {
                    let input: SessionsHistoryArgs = serde_json::from_value(args)?;
                    let messages = store.history(&input.session_id, input.limit).await;
                    Ok(serde_json::to_value(SessionsHistoryResult {
                        session_id: input.session_id,
                        messages,
                    })?)
                }
            },
        )
        .with_parameters_schema::<SessionsHistoryArgs>()
        .with_response_schema::<SessionsHistoryResult>(),
    ) as Arc<dyn Tool>;

    let math_add = Arc::new(
        FunctionTool::new(
            "math_add",
            "Add two numbers.",
            move |_ctx, args| async move {
                let input: MathAddArgs = serde_json::from_value(args)?;
                Ok(serde_json::to_value(MathAddResult {
                    sum: input.a + input.b,
                })?)
            },
        )
        .with_parameters_schema::<MathAddArgs>()
        .with_response_schema::<MathAddResult>(),
    ) as Arc<dyn Tool>;

    let time_now = Arc::new(
        FunctionTool::new(
            "time_now",
            "Return the current server time in Unix milliseconds and a simple UTC string.",
            move |_ctx, _args| async move {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let unix_ms = now.as_millis() as u64;
                Ok(serde_json::to_value(TimeNowResult {
                    unix_ms,
                    utc_hint: format_unix_ms(unix_ms),
                })?)
            },
        )
        .with_parameters_schema::<EmptyArgs>()
        .with_response_schema::<TimeNowResult>(),
    ) as Arc<dyn Tool>;

    let mut tools = vec![sessions_list, sessions_history, math_add, time_now];

    if let Some(english_learning) = english_learning.clone() {
        let learning_start = Arc::new(
            FunctionTool::new(
                "english_learning_start_today",
                "Start today's English learning lesson card for the current session.",
                move |ctx, _args| {
                    let learning = english_learning.clone();
                    async move {
                        let reply = learning
                            .start_today_lesson_tool(ctx.session_id())
                            .await
                            .map_err(|error| AdkError::Tool(error.to_string()))?;
                        Ok(serde_json::to_value(EnglishLearningReplyResult {
                            action: "english_learning_start_today".to_string(),
                            reply,
                        })?)
                    }
                },
            )
            .with_parameters_schema::<EmptyArgs>()
            .with_response_schema::<EnglishLearningReplyResult>(),
        ) as Arc<dyn Tool>;
        tools.push(learning_start);
    }

    if let Some(english_learning) = english_learning.clone() {
        let learning_explain = Arc::new(
            FunctionTool::new(
                "english_learning_explain_focus_sentence",
                "Explain the focus sentence from today's English learning lesson for the current session.",
                move |ctx, _args| {
                    let learning = english_learning.clone();
                    async move {
                        let reply = learning
                            .explain_focus_sentence_tool(ctx.session_id())
                            .await
                            .map_err(|error| AdkError::Tool(error.to_string()))?;
                        Ok(serde_json::to_value(EnglishLearningReplyResult {
                            action: "english_learning_explain_focus_sentence".to_string(),
                            reply,
                        })?)
                    }
                },
            )
            .with_parameters_schema::<EmptyArgs>()
            .with_response_schema::<EnglishLearningReplyResult>(),
        ) as Arc<dyn Tool>;
        tools.push(learning_explain);
    }

    if let Some(english_learning) = english_learning.clone() {
        let learning_next_question = Arc::new(
            FunctionTool::new(
                "english_learning_next_question",
                "Return the next comprehension question from today's English learning lesson for the current session.",
                move |ctx, _args| {
                    let learning = english_learning.clone();
                    async move {
                        let reply = learning
                            .next_question_tool(ctx.session_id())
                            .await
                            .map_err(|error| AdkError::Tool(error.to_string()))?;
                        Ok(serde_json::to_value(EnglishLearningReplyResult {
                            action: "english_learning_next_question".to_string(),
                            reply,
                        })?)
                    }
                },
            )
            .with_parameters_schema::<EmptyArgs>()
            .with_response_schema::<EnglishLearningReplyResult>(),
        ) as Arc<dyn Tool>;
        tools.push(learning_next_question);
    }

    if let Some(english_learning) = english_learning.clone() {
        let learning_shadowing_feedback = Arc::new(
            FunctionTool::new(
                "english_learning_shadowing_feedback",
                "Evaluate an English shadowing transcript against the current focus sentence for the session.",
                move |ctx, args| {
                    let learning = english_learning.clone();
                    async move {
                        let input: EnglishLearningShadowingArgs = serde_json::from_value(args)?;
                        let (handled, reply) = learning
                            .shadowing_feedback_tool(ctx.session_id(), &input.transcript)
                            .await
                            .map_err(|error| AdkError::Tool(error.to_string()))?;
                        Ok(serde_json::to_value(EnglishLearningShadowingResult {
                            handled,
                            reply,
                        })?)
                    }
                },
            )
            .with_parameters_schema::<EnglishLearningShadowingArgs>()
            .with_response_schema::<EnglishLearningShadowingResult>(),
        ) as Arc<dyn Tool>;
        tools.push(learning_shadowing_feedback);
    }

    if exec_command_config.enabled {
        let exec_command = Arc::new(
            FunctionTool::new(
                "exec_command",
                "Execute a shell command on the current server and return its exit code plus captured stdout/stderr.",
                move |_ctx, args| {
                    let config = exec_command_config.clone();
                    async move {
                        let input: ExecCommandArgs = serde_json::from_value(args)?;
                        let result = run_exec_command(input, &config)
                            .await
                            .map_err(|error| AdkError::Tool(error.to_string()))?;
                        Ok(serde_json::to_value(ExecCommandResult {
                            success: result.success,
                            exit_code: result.exit_code,
                            stdout: result.stdout,
                            stderr: result.stderr,
                            timed_out: result.timed_out,
                            command: result.command,
                            workdir: result.workdir,
                        })?)
                    }
                },
            )
            .with_parameters_schema::<ExecCommandArgs>()
            .with_response_schema::<ExecCommandResult>(),
        ) as Arc<dyn Tool>;
        tools.push(exec_command);
    }

    ToolRegistry::new(tools)
}
