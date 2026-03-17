# rs-tool-call

一个用 Rust 初始化的最小项目，目标是把 OpenClaw 的 tool call 核心链路落成一个清晰、可扩展的服务骨架：

1. 把工具注册成统一 schema。
2. 把 schema 传给 LLM。
3. 解析模型返回的 function/tool call。
4. 执行本地工具。
5. 把 tool result 重新喂回模型，直到得到最终回答。

HTTP 层使用 [salvo](https://github.com/salvo-rs/salvo)，LLM 集成使用 [adk-rust](https://github.com/zavora-ai/adk-rust)。

## 当前实现

- `POST /chat`
  - 走完整的 OpenClaw 风格 tool-call 循环。
  - 默认使用 `session_id = "main"` 持久化会话。
- `GET /tools`
  - 列出所有已注册工具及其 JSON Schema。
- `POST /tools/invoke`
  - 直接执行单个工具，接口形式参考 OpenClaw 的 `/tools/invoke`。
- `GET /sessions`
  - 查看当前内存中的会话。
- `GET /sessions/{session_id}/history`
  - 查看某个会话的消息历史。

## 内置工具

- `sessions_list`
- `sessions_history`
- `math_add`
- `time_now`

## 运行

先准备环境变量：

```bash
cp .env.example .env
```

OpenAI:

```bash
export LLM_PROVIDER=openai
export OPENAI_API_KEY=your-key
export LLM_MODEL=gpt-4o-mini
```

SiliconFlow GLM:

```bash
export LLM_PROVIDER=siliconflow
export SILICONFLOW_API_KEY=your-key
export LLM_MODEL=zai-org/GLM-4.6
# SiliconFlow 控制台 models 页面不是推理 API；服务端地址默认是 https://api.siliconflow.cn/v1
# export SILICONFLOW_BASE_URL=https://api.siliconflow.cn/v1
```

阿里百炼 GLM:

```bash
export LLM_PROVIDER=glm
export DASHSCOPE_API_KEY=your-key
export LLM_MODEL=glm-5
# 默认走北京 endpoint；如果你的 key 在其他地域，覆盖为对应地址
# export DASHSCOPE_BASE_URL=https://dashscope-intl.aliyuncs.com/compatible-mode/v1
# export DASHSCOPE_BASE_URL=https://dashscope-us.aliyuncs.com/compatible-mode/v1
```

Gemini:

```bash
cargo run --features gemini-provider
export LLM_PROVIDER=gemini
export GOOGLE_API_KEY=your-key
export LLM_MODEL=gemini-2.5-flash
```

启动：

```bash
cargo run
```

说明：

- `LLM_PROVIDER=siliconflow` 按 SiliconFlow 的 OpenAI 兼容接口处理，默认地址是 `https://api.siliconflow.cn/v1`。
- 如果你想切换到 SiliconFlow 的其他 GLM 型号，直接覆盖 `LLM_MODEL`，例如 `Pro/zai-org/GLM-5`。
- `LLM_PROVIDER=glm` 现在按阿里百炼的 OpenAI 兼容接口处理。
- 默认 `base_url` 是北京地域 `https://dashscope.aliyuncs.com/compatible-mode/v1`。
- 你也可以用 `DASHSCOPE_BASE_URL`、`BAILIAN_BASE_URL`、`GLM_BASE_URL` 覆盖地域 endpoint。
- API key 支持 `DASHSCOPE_API_KEY`、`BAILIAN_API_KEY`、`GLM_API_KEY`、`LLM_API_KEY`。

## 请求示例

完整 tool-call：

```bash
curl -s http://127.0.0.1:7878/chat \
  -H 'content-type: application/json' \
  -d '{
    "session_id": "main",
    "message": "帮我调用 math_add 计算 19 + 23，然后告诉我结果"
  }'
```

直接调用工具：

```bash
curl -s http://127.0.0.1:7878/tools/invoke \
  -H 'content-type: application/json' \
  -d '{
    "tool": "sessions_list",
    "args": {
      "limit": 10
    }
 }'
```

独立的回溯调用最小示例：

```bash
cargo run --example backtracking_call
```

这个示例放在 `examples/backtracking_call.rs`，不会影响现有 HTTP 服务结构。
它演示的是：

- 上层是回溯搜索器，默认加载 3 个可扩展的思考方向。
- 下层每个思考方向内部仍然是线性的 tool loop。
- 每个分支都在独立 sandbox 里执行，失败就回滚，成功才提交副作用。
- 执行反馈会触发 `success` / `dead_end` / `pruned` 三种结果，用来剪枝和切换分支。

## 代码结构

- `src/engine.rs`: OpenClaw 风格的 tool-call 编排循环。
- `src/tools.rs`: 工具注册、schema 暴露、执行上下文。
- `src/http_api.rs`: Salvo HTTP 路由与请求处理。
- `src/models.rs`: adk-rust 模型初始化。
- `src/session_store.rs`: 内存会话存储和历史视图。

## 后续可扩展方向

- 加入 tool policy / auth / allowlist。
- 增加流式输出和 SSE。
- 补上 provider-specific tool call ID 兼容层。
- 接入真正的持久化 session store。
