use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
};

use adk_rust::{
    AdkError, CallbackContext, Content, EventActions, ReadonlyContext, Tool, ToolContext,
    async_trait,
    serde_json::{Value, json},
    tool::FunctionTool,
};
use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time::timeout};

use crate::{
    config::ExecCommandToolConfig,
    session_store::{MessageView, SessionStore, SessionSummary},
};

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<HashMap<String, Arc<dyn Tool>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters_schema: Value,
    pub response_schema: Option<Value>,
    pub long_running: bool,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionRequest {
    pub app_name: String,
    pub user_id: String,
    pub session_id: String,
    pub invocation_id: String,
    pub function_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub user_content: Content,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub function_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub output: Value,
    #[allow(dead_code)]
    pub actions: EventActions,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolExecutionFailure {
    pub function_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SessionsListArgs {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SessionsHistoryArgs {
    session_id: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SessionsHistoryResult {
    session_id: String,
    messages: Vec<MessageView>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct MathAddArgs {
    a: f64,
    b: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct MathAddResult {
    sum: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct TimeNowResult {
    unix_ms: u64,
    utc_hint: String,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
struct EmptyArgs {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ExecCommandArgs {
    cmd: String,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    max_output_chars: Option<usize>,
    #[serde(default)]
    shell: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ExecCommandResult {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    command: String,
    workdir: Option<String>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Result<Self> {
        let mut map = HashMap::new();
        for tool in tools {
            let name = tool.name().to_string();
            if map.insert(name.clone(), tool).is_some() {
                bail!("duplicate tool registration: {name}");
            }
        }

        Ok(Self {
            tools: Arc::new(map),
        })
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut items = self
            .tools
            .values()
            .map(|tool| ToolDescriptor {
                name: tool.name().to_string(),
                description: tool.enhanced_description(),
                parameters_schema: tool
                    .parameters_schema()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                response_schema: tool.response_schema(),
                long_running: tool.is_long_running(),
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.name.cmp(&right.name));
        items
    }

    pub fn schemas(&self) -> HashMap<String, Value> {
        self.tools
            .values()
            .map(|tool| {
                (
                    tool.name().to_string(),
                    json!({
                        "description": tool.enhanced_description(),
                        "parameters": tool.parameters_schema().unwrap_or_else(|| json!({
                            "type": "object",
                            "properties": {}
                        })),
                    }),
                )
            })
            .collect()
    }

    pub fn has(&self, tool_name: &str) -> bool {
        self.tools.contains_key(tool_name)
    }

    pub async fn execute(&self, request: ToolExecutionRequest) -> Result<ToolExecutionResult> {
        let tool = self
            .tools
            .get(&request.tool_name)
            .cloned()
            .ok_or_else(|| anyhow!("tool not found: {}", request.tool_name))?;
        let context = Arc::new(RequestToolContext::new(&request));
        let output = tool.execute(context.clone(), request.args.clone()).await?;

        Ok(ToolExecutionResult {
            function_call_id: request.function_call_id,
            tool_name: request.tool_name,
            args: request.args,
            output,
            actions: context.actions(),
        })
    }
}

pub fn build_builtin_registry(
    session_store: SessionStore,
    exec_command_config: ExecCommandToolConfig,
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
                        Ok(serde_json::to_value(result)?)
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

async fn run_exec_command(
    input: ExecCommandArgs,
    config: &ExecCommandToolConfig,
) -> Result<ExecCommandResult> {
    let cmd = input.cmd.trim();
    if cmd.is_empty() {
        bail!("cmd must not be empty");
    }

    let shell = input
        .shell
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(config.shell.as_str());
    let timeout_secs = input
        .timeout_secs
        .unwrap_or(config.timeout_secs)
        .clamp(1, 120);
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(config.max_output_chars)
        .clamp(128, 20_000);

    let mut command = Command::new(shell);
    command
        .arg("-lc")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(workdir) = input
        .workdir
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        command.current_dir(workdir);
    }

    let child = command.spawn()?;
    let output = match timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Ok(ExecCommandResult {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("command timed out after {timeout_secs}s"),
                timed_out: true,
                command: cmd.to_string(),
                workdir: input.workdir,
            });
        }
    };

    Ok(ExecCommandResult {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: truncate_output(&String::from_utf8_lossy(&output.stdout), max_output_chars),
        stderr: truncate_output(&String::from_utf8_lossy(&output.stderr), max_output_chars),
        timed_out: false,
        command: cmd.to_string(),
        workdir: input.workdir,
    })
}

fn format_unix_ms(unix_ms: u64) -> String {
    let seconds = unix_ms / 1_000;
    let millis = unix_ms % 1_000;
    format!("{seconds}.{millis:03}Z")
}

fn truncate_output(value: &str, max_chars: usize) -> String {
    let mut truncated = value.trim().chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

struct RequestToolContext {
    invocation_id: String,
    agent_name: String,
    user_id: String,
    app_name: String,
    session_id: String,
    branch: String,
    user_content: Content,
    function_call_id: String,
    actions: Mutex<EventActions>,
}

impl RequestToolContext {
    fn new(request: &ToolExecutionRequest) -> Self {
        Self {
            invocation_id: request.invocation_id.clone(),
            agent_name: "tool-call-engine".to_string(),
            user_id: request.user_id.clone(),
            app_name: request.app_name.clone(),
            session_id: request.session_id.clone(),
            branch: String::new(),
            user_content: request.user_content.clone(),
            function_call_id: request.function_call_id.clone(),
            actions: Mutex::new(EventActions::default()),
        }
    }
}

#[async_trait]
impl ReadonlyContext for RequestToolContext {
    fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    fn agent_name(&self) -> &str {
        &self.agent_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn branch(&self) -> &str {
        &self.branch
    }

    fn user_content(&self) -> &Content {
        &self.user_content
    }
}

#[async_trait]
impl CallbackContext for RequestToolContext {
    fn artifacts(&self) -> Option<Arc<dyn adk_rust::Artifacts>> {
        None
    }
}

#[async_trait]
impl ToolContext for RequestToolContext {
    fn function_call_id(&self) -> &str {
        &self.function_call_id
    }

    fn actions(&self) -> EventActions {
        self.actions.lock().expect("tool actions poisoned").clone()
    }

    fn set_actions(&self, actions: EventActions) {
        *self.actions.lock().expect("tool actions poisoned") = actions;
    }

    async fn search_memory(
        &self,
        _query: &str,
    ) -> std::result::Result<Vec<adk_rust::MemoryEntry>, AdkError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exec_command_runs_and_captures_output() {
        let result = run_exec_command(
            ExecCommandArgs {
                cmd: "printf 'hello'".to_string(),
                workdir: None,
                timeout_secs: Some(5),
                max_output_chars: Some(100),
                shell: Some("/bin/sh".to_string()),
            },
            &ExecCommandToolConfig {
                enabled: true,
                shell: "/bin/sh".to_string(),
                timeout_secs: 20,
                max_output_chars: 4000,
            },
        )
        .await
        .expect("command should succeed");

        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn truncate_output_limits_text() {
        assert_eq!(truncate_output("abcdef", 4), "abcd...");
        assert_eq!(truncate_output("abc", 4), "abc");
    }
}
