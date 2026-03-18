use std::{
    collections::HashMap,
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

use crate::session_store::{MessageView, SessionStore, SessionSummary};

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

pub fn build_builtin_registry(session_store: SessionStore) -> Result<ToolRegistry> {
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

    ToolRegistry::new(vec![sessions_list, sessions_history, math_add, time_now])
}

fn format_unix_ms(unix_ms: u64) -> String {
    let seconds = unix_ms / 1_000;
    let millis = unix_ms % 1_000;
    format!("{seconds}.{millis:03}Z")
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
