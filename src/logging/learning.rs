//! 英语学习日志模块，负责记录每日新闻抓取、学习卡片生成和飞书学习口令处理过程。

use std::path::Path;

use tracing::{error, info};

use super::preview_text;

/// 记录英语学习能力配置。
pub fn log_learning_config(
    enabled: bool,
    scheduler_enabled: bool,
    schedule_hour: u32,
    timezone_offset_hours: i32,
    storage_dir: &Path,
    news_source_count: usize,
) {
    info!(
        english_learning_enabled = enabled,
        english_learning_scheduler_enabled = scheduler_enabled,
        english_learning_schedule_hour = schedule_hour,
        english_learning_timezone_offset_hours = timezone_offset_hours,
        english_learning_storage_dir = %storage_dir.display(),
        english_learning_news_source_count = news_source_count,
        "loaded english learning config"
    );
}

/// 记录调度器开始运行。
pub fn log_learning_scheduler_started(schedule_hour: u32, timezone_offset_hours: i32) {
    info!(
        schedule_hour,
        timezone_offset_hours, "started english learning scheduler"
    );
}

/// 记录开始抓取今日新闻。
pub fn log_learning_news_ingest_started(lesson_date: &str, news_source_count: usize) {
    info!(
        lesson_date = %lesson_date,
        news_source_count,
        "starting daily english news ingest"
    );
}

/// 记录选中的新闻条目。
pub fn log_learning_news_selected(lesson_date: &str, title: &str, link: &str) {
    info!(
        lesson_date = %lesson_date,
        title_preview = %preview_text(title, 160),
        link = %link,
        "selected news item for daily english lesson"
    );
}

/// 记录学习卡片落盘。
pub fn log_learning_lesson_saved(
    lesson_date: &str,
    path: &Path,
    question_count: usize,
    vocabulary_count: usize,
) {
    info!(
        lesson_date = %lesson_date,
        path = %path.display(),
        question_count,
        vocabulary_count,
        "saved daily english lesson card"
    );
}

/// 记录命中英语学习口令。
pub fn log_learning_command_handled(session_id: &str, command: &str, lesson_date: &str) {
    info!(
        session_id = %session_id,
        command = %command,
        lesson_date = %lesson_date,
        "handled english learning command"
    );
}

/// 记录英语学习后台任务失败。
pub fn log_learning_background_error(stage: &str, error_message: &str) {
    error!(stage = %stage, error = %error_message, "english learning background task failed");
}
