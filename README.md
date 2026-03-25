# mimic-shrimp-rs

仿照龙虾，搞了一个基于 Rust 的英语学习助手服务，当前重点是把“每日英语学习卡片 + 飞书互动”这条链路跑通。

## 现在能做什么

- 每天抓取固定 RSS 新闻源，生成一份“今日英语学习卡片”。
- 学习卡片包含中英摘要、重点词汇、重点句子、理解问题、跟读练习和翻译练习。
- 支持在同一会话里继续追问重点句、继续做下一题，以及对英语跟读文本给出反馈。
- 学习内容默认落盘保存，便于按天复用和追踪。

## 飞书接入状态

- 已接通飞书回调入口：`POST /feishu/callback`。
- 已支持接收飞书文本消息事件，并异步调用现有能力后回复原消息。
- 已支持飞书学习口令优先命中英语学习流程，而不是普通聊天。
- 当前已支持的学习口令包括：`开始今天的英语学习`、`这句话什么意思`、`再出一道题`。

## 飞书对话示例

![飞书英语学习对话示例 1](images/1.png)

![飞书英语学习对话示例 2](images/2.png)

## 内置英语学习工具

- `english_learning_start_today`
- `english_learning_explain_focus_sentence`
- `english_learning_next_question`
- `english_learning_shadowing_feedback`

## 快速运行

```bash
cp .env.example .env
cargo run
```

如果你要启用飞书完整消息收发，至少需要配置：

- `FEISHU_APP_ID`
- `FEISHU_APP_SECRET`
- `FEISHU_CALLBACK_VERIFICATION_TOKEN`（如果飞书后台开启了 token 校验）
- `FEISHU_CALLBACK_ENCRYPT_KEY`（如果飞书后台开启了加密策略）

英语学习能力默认启用，可通过以下环境变量调整：

- `ENGLISH_LEARNING_ENABLED`
- `ENGLISH_LEARNING_SCHEDULER_ENABLED`
- `ENGLISH_LEARNING_SCHEDULE_HOUR`
- `ENGLISH_LEARNING_TZ_OFFSET_HOURS`
- `ENGLISH_LEARNING_NEWS_SOURCES`

## 详细说明

完整接口、部署、工具列表和请求示例见 [detail.md](detail.md)。
