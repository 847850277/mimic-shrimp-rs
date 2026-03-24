//! 英语学习配置模块，负责声明每日新闻抓取和学习卡片生成所需的运行参数。

use std::path::PathBuf;

/// 每日英语学习能力配置。
#[derive(Debug, Clone)]
pub struct EnglishLearningConfig {
    pub enabled: bool,
    pub scheduler_enabled: bool,
    pub schedule_hour: u32,
    pub timezone_offset_hours: i32,
    pub storage_dir: PathBuf,
    pub news_sources: Vec<String>,
    pub max_feed_items_per_source: usize,
}
