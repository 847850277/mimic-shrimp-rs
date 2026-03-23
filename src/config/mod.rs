//! `config` 模块负责统一读取环境变量并构造应用运行配置。
//! 这里将应用级配置、LLM 配置、通道配置、工具配置和环境解析辅助逻辑拆成独立子模块。

mod app;
mod channel;
mod env;
mod form;
mod llm;
mod tool;

pub use app::AppConfig;
pub use channel::FeishuCallbackConfig;
pub use form::FormConfig;
pub use llm::{LlmConfig, LlmProvider};
pub use tool::ExecCommandToolConfig;
