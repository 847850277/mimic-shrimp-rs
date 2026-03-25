# mimic-shrimp-rs

一个用 Rust 初始化的最小项目，目标是把 OpenClaw 的 tool call 核心链路落成一个清晰、可扩展的服务骨架：

1. 把工具注册成统一 schema。
2. 让 LLM 每轮只规划下一步 action，而不是整条长链。
3. 从最多 3 个候选方向里选一个可提交 action。
4. 执行这一步并观察结果。
5. 把 observation 回写到 transcript，再继续 re-plan，直到得到最终回答。

HTTP 层使用 [salvo](https://github.com/salvo-rs/salvo)，LLM 集成使用 [adk-rust](https://github.com/zavora-ai/adk-rust)。

## 当前实现

- `POST /chat`
  - 走 `Plan Next -> Execute One -> Observe -> Replan` 的迭代式主循环。
  - 每轮最多保留 3 个候选方向，但只 commit 1 个 action。
  - 内置 `max_iterations`、`max_tool_calls_per_turn`、repeated-call detection、error budget 这几类 guard。
  - 响应体会返回 `planning_steps`，方便调试“为什么选这一步”。
  - 默认使用 `session_id = "main"` 持久化会话。
- `POST /feishu/callback`
  - 接收飞书卡片回调和 `im.message.receive_v1` 文本消息事件。
  - 支持明文请求、`challenge` 校验回包、可选的 `encrypt` 解密。
  - 如果配置了 `FEISHU_CALLBACK_VERIFICATION_TOKEN`，会校验 payload 中的 token。
  - 文本消息事件会异步调用现有 `ToolCallEngine`，再通过飞书开放平台 reply API 回复原消息。
- `GET /tools`
  - 列出所有已注册工具及其 JSON Schema。
- `POST /tools/invoke`
  - 直接执行单个工具，接口形式参考 OpenClaw 的 `/tools/invoke`。
- `GET /sessions`
  - 查看当前内存中的会话。
- `GET /sessions/{session_id}/history`
  - 查看某个会话的消息历史。
- `POST /extract/form`
  - 独立于 `/chat` 的单轮结构化抽取接口。
  - 支持通过 `form_id` 从本地 Markdown 表单目录加载字段定义，也支持直接传 `schema`。
  - 模型抽取完成后会按 schema 做二次校验，返回 `missing_fields`、`invalid_fields` 和 `warnings`。
- `POST /translate/media`
  - 独立于 `/chat` 的媒体翻译接口。
  - 当前对接阿里百炼 `qwen3-livetranslate-flash` 的 OpenAI 兼容接口。
  - 支持音频 URL/Data URL 或视频 URL 输入，默认返回文本翻译，也支持可选音频输出。
- `POST /speech/synthesize`
  - 独立于 `/chat` 的文本转语音接口。
  - 当前对接 SiliconFlow `POST /audio/speech` 接口。
  - 支持通过文本生成音频，返回 base64 音频数据和内容类型，便于后续接飞书音频回发。
- 每日英语学习能力
  - 服务内可选调度器会在每天早上 9 点拉取固定 RSS 新闻源，生成一份“今日英语学习卡片”并保存到本地。
  - 当前卡片包含：中英摘要、重点词汇、重点句子、3 个理解问题、跟读练习、翻译练习。
  - 飞书里命中以下口令时，会优先走学习能力而不是普通聊天：
    - `开始今天的英语学习`
    - `这句话什么意思`
    - `再出一道题`

## 内置工具

- `sessions_list`
- `sessions_history`
- `math_add`
- `time_now`
- `english_learning_start_today`（默认启用，可通过 `ENGLISH_LEARNING_ENABLED=false` 关闭）
  - 为当前会话启动“今日英语学习卡片”。
- `english_learning_explain_focus_sentence`（默认启用，可通过 `ENGLISH_LEARNING_ENABLED=false` 关闭）
  - 解释当前学习卡片里的重点句子。
- `english_learning_next_question`（默认启用，可通过 `ENGLISH_LEARNING_ENABLED=false` 关闭）
  - 返回当前学习卡片里的下一道理解题。
- `english_learning_shadowing_feedback`（默认启用，可通过 `ENGLISH_LEARNING_ENABLED=false` 关闭）
  - 对当前会话的英语跟读文本做评估；参数中需要提供 `transcript`。
- `exec_command`（可选，需要 `EXEC_COMMAND_TOOL_ENABLED=true`）
  - 对连续 shell 调用加了收敛保护：拿到可用结果后优先直接回答，不再无限探测环境。

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

SiliconFlow TTS:

```bash
export SPEECH_SYNTHESIS_API_KEY=your-key
# 默认走 SiliconFlow TTS endpoint
# export SPEECH_SYNTHESIS_BASE_URL=https://api.siliconflow.cn/v1
export SPEECH_SYNTHESIS_MODEL=MOSS-TTSD-v0.5
export SPEECH_SYNTHESIS_VOICE=alex
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

Docker 构建：

```bash
docker build -t mimic-shrimp-rs:latest .
```

如果你要打带 Gemini provider 的镜像：

```bash
docker build --build-arg APP_FEATURES=gemini-provider -t mimic-shrimp-rs:gemini .
```

Docker 运行：

```bash
docker run -d \
  --name mimic-shrimp-rs \
  --restart unless-stopped \
  -p 7878:7878 \
  --env-file .env \
  -e SERVER_ADDR=0.0.0.0:7878 \
  mimic-shrimp-rs:latest
```

一键更新部署：

```bash
bash scripts/deploy.sh
```

这个脚本会按当前分支执行 `git pull --ff-only`、重新构建镜像，并替换当前运行中的 `mimic-shrimp-rs` 容器；默认读取项目根目录下的 `.env`。

如果你要推到镜像仓库：

```bash
docker tag mimic-shrimp-rs:latest your-registry/mimic-shrimp-rs:latest
docker push your-registry/mimic-shrimp-rs:latest
```

说明：

- `LLM_PROVIDER=siliconflow` 按 SiliconFlow 的 OpenAI 兼容接口处理，默认地址是 `https://api.siliconflow.cn/v1`。
- 如果你想切换到 SiliconFlow 的其他 GLM 型号，直接覆盖 `LLM_MODEL`，例如 `Pro/zai-org/GLM-5`。
- `LLM_PROVIDER=glm` 现在按阿里百炼的 OpenAI 兼容接口处理。
- 默认只会把最近 `20` 条历史消息重新带入模型上下文，可通过 `MAX_CONTEXT_MESSAGES` 调整。
- 默认 `base_url` 是北京地域 `https://dashscope.aliyuncs.com/compatible-mode/v1`。
- 你也可以用 `DASHSCOPE_BASE_URL`、`BAILIAN_BASE_URL`、`GLM_BASE_URL` 覆盖地域 endpoint。
- API key 支持 `DASHSCOPE_API_KEY`、`BAILIAN_API_KEY`、`GLM_API_KEY`、`LLM_API_KEY`。
- Docker 镜像默认监听 `0.0.0.0:7878`，容器内直接运行 `mimic-shrimp-rs`。
- 如果你在飞书后台配置服务端回调地址，建议填 `https://your-domain/feishu/callback` 或 `https://your-domain/api/feishu/callback`。
- 如果你开启飞书加密策略，需要同时配置 `FEISHU_CALLBACK_ENCRYPT_KEY`。
- 如果你配置了 Verification Token，需要同时配置 `FEISHU_CALLBACK_VERIFICATION_TOKEN`。
- 如果你要让机器人在群里收到文本并自动回复，还需要配置 `FEISHU_APP_ID`、`FEISHU_APP_SECRET`。
- 飞书后台除了“回调配置”，还要在“事件配置”里订阅 `im.message.receive_v1`，并在权限管理里开通接收/回复消息相关权限。
- 默认 `FEISHU_BOT_REQUIRE_MENTION=true`，群聊里只有显式 `@机器人` 的文本消息才会触发回复；点对点聊天不受这个限制。
- 如果你要让模型能在服务器上执行 shell 命令，需要显式开启 `EXEC_COMMAND_TOOL_ENABLED=true`；默认关闭。
- 本地 Markdown 表单目录默认是 `./forms`，可用 `FORM_MARKDOWN_DIR` 覆盖。
- 媒体翻译接口默认读取 `MEDIA_TRANSLATE_API_KEY` 和 `MEDIA_TRANSLATE_BASE_URL`；未单独配置时，会回退到 DashScope 相关环境变量。
- 语音合成接口默认读取 `SPEECH_SYNTHESIS_API_KEY` 和 `SPEECH_SYNTHESIS_BASE_URL`；未单独配置时，会回退到 SiliconFlow 相关环境变量。
- 每日英语学习能力默认使用 `./learning_data/lessons` 保存 lesson 文件，可用 `ENGLISH_LEARNING_STORAGE_DIR` 覆盖。
- 每日英语学习调度默认按 `UTC+8` 的 `09:00` 运行，可通过 `ENGLISH_LEARNING_SCHEDULE_HOUR` 和 `ENGLISH_LEARNING_TZ_OFFSET_HOURS` 调整。
- 新闻源默认是 `https://feeds.bbci.co.uk/news/world/rss.xml`，也可以通过 `ENGLISH_LEARNING_NEWS_SOURCES` 配多个 RSS 地址，逗号分隔。

## 请求示例

完整 tool-call：

```bash
curl -s http://127.0.0.1:7878/chat \
  -H 'content-type: application/json' \
  -d '{
    "session_id": "main",
    "message": "获取一下成都的天气"
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

结构化表单抽取：

```bash
curl -s http://127.0.0.1:7878/extract/form \
  -H 'content-type: application/json' \
  -d '{
    "form_id": "basic_profile",
    "text": "我叫张三，男，32岁，手机号 13800138000"
  }'
```

本地 Markdown 表单示例见 [forms/basic_profile.md](/Users/zhengpeng/Source/Code/Rust-Code/Github/mimic-shrimp-rs/forms/basic_profile.md)。

媒体翻译：

```bash
curl -s http://127.0.0.1:7878/translate/media \
  -H 'content-type: application/json' \
  -d '{
    "source_lang": "Chinese",
    "target_lang": "English",
    "audio": {
      "data": "https://dashscope.oss-cn-beijing.aliyuncs.com/audios/welcome.wav",
      "format": "wav"
    }
  }'
```

语音合成：

```bash
curl -s http://127.0.0.1:7878/speech/synthesize \
  -H 'content-type: application/json' \
  -d '{
    "text": "Hello, welcome to today'\''s English lesson.",
    "voice": "alex",
    "response_format": "mp3"
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

- `src/engine.rs` + `src/engine/*`: Iterative plan-execute loop，负责 planner / selector / guard / executor 的主编排。
- `src/tools/*`: 工具注册、schema 暴露、执行上下文。
- `src/web/*`: Salvo HTTP 路由与请求处理。
- `src/capability/*`: 聊天、工具调用、会话查询、结构化抽取等能力边界。
- `src/capability/media_translate.rs`: 阿里百炼媒体翻译能力，独立调用兼容接口。
- `src/forms/*`: 本地 Markdown 表单加载与抽取结果校验。
- `src/models/*`: adk-rust 模型初始化。
- `src/session_store.rs`: 内存会话存储和历史视图。

## 后续可扩展方向

- 加入 tool policy / auth / allowlist。
- 增加流式输出和 SSE。
- 补上 provider-specific tool call ID 兼容层。
- 接入真正的持久化 session store。
