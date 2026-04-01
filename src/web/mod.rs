//! `web` 模块负责 HTTP 接入层。
//! 这里拆分了路由、状态、请求/响应结构以及各个 handler，便于后续继续扩展 Web 能力。

mod cors;
mod handlers;
mod pages;
mod router;
mod state;
mod types;
mod util;

pub use router::run_http;
pub use state::AppState;
