use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize)]
struct SearchProblem {
    start: i64,
    operands: Vec<i64>,
    target: i64,
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceState {
    current: i64,
    cursor: usize,
    pending_effects: Vec<String>,
}

impl WorkspaceState {
    fn new(start: i64) -> Self {
        Self {
            current: start,
            cursor: 0,
            pending_effects: Vec::new(),
        }
    }

    fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            current: self.current,
            cursor: self.cursor,
            pending_effects: self.pending_effects.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceSnapshot {
    current: i64,
    cursor: usize,
    pending_effects: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ToolCallTrace {
    step: usize,
    tool: String,
    args: Value,
    output: Value,
    observation: String,
}

#[derive(Debug, Clone, Serialize)]
struct BranchAttempt {
    direction: String,
    thought: String,
    planned_tools: Vec<String>,
    status: String,
    decision: String,
    reason: String,
    sandbox_before: WorkspaceSnapshot,
    sandbox_after: WorkspaceSnapshot,
    tool_calls: Vec<ToolCallTrace>,
}

#[derive(Debug, Clone, Serialize)]
struct SearchReport {
    loaded_directions: Vec<String>,
    problem: SearchProblem,
    attempts: Vec<BranchAttempt>,
    final_result: FinalResult,
}

#[derive(Debug, Clone, Serialize)]
struct FinalResult {
    status: String,
    committed_state: WorkspaceSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Add,
    Mul,
}

impl ToolKind {
    fn as_name(self) -> &'static str {
        match self {
            ToolKind::Add => "math_add",
            ToolKind::Mul => "math_mul",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ThoughtDirection {
    name: &'static str,
    thought: &'static str,
    planner: fn(usize) -> Vec<ToolKind>,
}

impl ThoughtDirection {
    fn plan(self, remaining_operands: usize) -> Vec<ToolKind> {
        (self.planner)(remaining_operands)
    }
}

struct ThoughtDirectionRegistry {
    items: Vec<ThoughtDirection>,
}

impl Default for ThoughtDirectionRegistry {
    fn default() -> Self {
        let mut registry = Self { items: Vec::new() };
        registry.register(ThoughtDirection {
            name: "sum-first",
            thought: "先走最保守的加法链路，验证不依赖复杂切换时能否命中目标。",
            planner: plan_add_all,
        });
        registry.register(ThoughtDirection {
            name: "multiply-first",
            thought: "优先尝试乘法放大，如果执行反馈明显偏离目标就立刻剪枝回溯。",
            planner: plan_mul_all,
        });
        registry.register(ThoughtDirection {
            name: "bridge-plan",
            thought: "先加再乘再加，把中间结果桥接到目标值，作为默认第三个思考方向。",
            planner: plan_add_mul_add,
        });
        registry
    }
}

impl ThoughtDirectionRegistry {
    fn register(&mut self, direction: ThoughtDirection) {
        self.items.push(direction);
    }

    fn directions(&self) -> &[ThoughtDirection] {
        &self.items
    }

    fn names(&self) -> Vec<String> {
        self.items.iter().map(|item| item.name.to_string()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchStatus {
    Success,
    DeadEnd,
    Pruned,
}

impl BranchStatus {
    fn as_str(self) -> &'static str {
        match self {
            BranchStatus::Success => "success",
            BranchStatus::DeadEnd => "dead_end",
            BranchStatus::Pruned => "pruned",
        }
    }

    fn decision(self) -> &'static str {
        match self {
            BranchStatus::Success => "commit",
            BranchStatus::DeadEnd | BranchStatus::Pruned => "rollback",
        }
    }
}

struct BranchOutcome {
    status: BranchStatus,
    reason: String,
    tool_calls: Vec<ToolCallTrace>,
}

fn main() -> Result<()> {
    let problem = SearchProblem {
        start: 2,
        operands: vec![3, 4, 5],
        target: 25,
    };
    let registry = ThoughtDirectionRegistry::default();
    let mut committed_state = WorkspaceState::new(problem.start);
    let mut attempts = Vec::new();

    let found = backtrack_search(
        &problem,
        registry.directions(),
        0,
        &mut committed_state,
        &mut attempts,
    )?;

    let report = SearchReport {
        loaded_directions: registry.names(),
        problem,
        attempts,
        final_result: FinalResult {
            status: if found {
                "found_plan".to_string()
            } else {
                "not_found".to_string()
            },
            committed_state: committed_state.snapshot(),
        },
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn backtrack_search(
    problem: &SearchProblem,
    directions: &[ThoughtDirection],
    index: usize,
    committed_state: &mut WorkspaceState,
    attempts: &mut Vec<BranchAttempt>,
) -> Result<bool> {
    if index >= directions.len() {
        return Ok(false);
    }

    let direction = directions[index];
    let mut sandbox = committed_state.clone();
    let sandbox_before = sandbox.snapshot();
    let planned_tools = direction.plan(problem.operands.len().saturating_sub(sandbox.cursor));
    let outcome = run_linear_tool_loop(problem, &mut sandbox, &planned_tools)?;

    attempts.push(BranchAttempt {
        direction: direction.name.to_string(),
        thought: direction.thought.to_string(),
        planned_tools: planned_tools
            .iter()
            .map(|tool| tool.as_name().to_string())
            .collect(),
        status: outcome.status.as_str().to_string(),
        decision: outcome.status.decision().to_string(),
        reason: outcome.reason,
        sandbox_before,
        sandbox_after: sandbox.snapshot(),
        tool_calls: outcome.tool_calls,
    });

    if outcome.status == BranchStatus::Success {
        *committed_state = sandbox;
        return Ok(true);
    }

    backtrack_search(problem, directions, index + 1, committed_state, attempts)
}

fn run_linear_tool_loop(
    problem: &SearchProblem,
    sandbox: &mut WorkspaceState,
    plan: &[ToolKind],
) -> Result<BranchOutcome> {
    let mut tool_calls = Vec::new();

    for (step_index, tool) in plan.iter().enumerate() {
        if sandbox.cursor >= problem.operands.len() {
            break;
        }

        let operand = problem.operands[sandbox.cursor];
        let current = sandbox.current;
        let args = json!({
            "a": current,
            "b": operand,
        });
        let output = invoke_tool(tool.as_name(), &args)?;
        let next = extract_value(&output)?;

        sandbox.pending_effects.push(format!(
            "{}({}, {}) = {}",
            tool.as_name(),
            current,
            operand,
            next
        ));
        sandbox.current = next;
        sandbox.cursor += 1;

        let remaining = problem.operands.len().saturating_sub(sandbox.cursor);
        let (status, observation) = inspect_execution(problem, sandbox.current, remaining);
        tool_calls.push(ToolCallTrace {
            step: step_index + 1,
            tool: tool.as_name().to_string(),
            args,
            output,
            observation,
        });

        if let Some(status) = status {
            return Ok(BranchOutcome {
                status,
                reason: branch_reason(problem, sandbox, status),
                tool_calls,
            });
        }
    }

    let status = if sandbox.current == problem.target {
        BranchStatus::Success
    } else {
        BranchStatus::DeadEnd
    };

    Ok(BranchOutcome {
        status,
        reason: branch_reason(problem, sandbox, status),
        tool_calls,
    })
}

fn inspect_execution(
    problem: &SearchProblem,
    current: i64,
    remaining_operands: usize,
) -> (Option<BranchStatus>, String) {
    if current == problem.target {
        return (
            Some(BranchStatus::Success),
            format!("命中目标 {}，当前分支可以提交。", problem.target),
        );
    }

    if current.abs() > problem.target.abs().max(1) * 4 {
        return (
            Some(BranchStatus::Pruned),
            format!(
                "当前值 {} 明显偏离目标 {}，剪枝并回溯到下一个思考方向。",
                current, problem.target
            ),
        );
    }

    if remaining_operands == 0 {
        return (
            Some(BranchStatus::DeadEnd),
            format!(
                "线性 tool loop 已走完，但当前值 {} 仍未命中目标 {}。",
                current, problem.target
            ),
        );
    }

    (
        None,
        format!(
            "保留当前思路继续执行；当前值 {}，距离目标 {} 还有 {} 个操作数可用。",
            current, problem.target, remaining_operands
        ),
    )
}

fn branch_reason(problem: &SearchProblem, sandbox: &WorkspaceState, status: BranchStatus) -> String {
    match status {
        BranchStatus::Success => format!(
            "分支命中目标 {}，提交副作用并保留当前调用链。",
            problem.target
        ),
        BranchStatus::DeadEnd => format!(
            "分支执行完成但停在 {}，未命中目标 {}，回滚副作用。",
            sandbox.current, problem.target
        ),
        BranchStatus::Pruned => format!(
            "分支中途被剪枝，当前值 {} 已明显偏离目标 {}，回滚副作用。",
            sandbox.current, problem.target
        ),
    }
}

fn plan_add_all(remaining_operands: usize) -> Vec<ToolKind> {
    vec![ToolKind::Add; remaining_operands]
}

fn plan_mul_all(remaining_operands: usize) -> Vec<ToolKind> {
    vec![ToolKind::Mul; remaining_operands]
}

fn plan_add_mul_add(remaining_operands: usize) -> Vec<ToolKind> {
    let pattern = [ToolKind::Add, ToolKind::Mul, ToolKind::Add];
    (0..remaining_operands)
        .map(|index| pattern[index % pattern.len()])
        .collect()
}

fn invoke_tool(tool: &str, args: &Value) -> Result<Value> {
    let a = args
        .get("a")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing integer arg: a"))?;
    let b = args
        .get("b")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing integer arg: b"))?;

    match tool {
        "math_add" => Ok(json!({ "value": a + b })),
        "math_mul" => Ok(json!({ "value": a * b })),
        other => bail!("unknown tool: {other}"),
    }
}

fn extract_value(output: &Value) -> Result<i64> {
    output
        .get("value")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("tool output missing integer field: value"))
}
