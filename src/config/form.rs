//! 表单配置模块，负责声明本地 Markdown 表单目录等抽取相关配置。

use std::path::PathBuf;

/// 结构化抽取使用的表单仓库配置。
#[derive(Debug, Clone)]
pub struct FormConfig {
    pub markdown_dir: PathBuf,
}
