//! 环境变量解析辅助模块，负责统一处理多 key 回退和基础类型转换。

use anyhow::{Result, anyhow};

/// 按顺序读取多个环境变量，返回第一个存在的值。
pub(crate) fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

/// 解析布尔环境变量，支持常见的开关表示法。
pub(crate) fn parse_bool_env(key: &str, default: bool) -> Result<bool> {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(anyhow!("unsupported boolean value: {raw}")),
        },
        Err(_) => Ok(default),
    }
}

/// 解析无符号 64 位整数环境变量。
pub(crate) fn parse_u64_env(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse::<u64>()
            .map_err(|_| anyhow!("unsupported integer value: {raw}")),
        Err(_) => Ok(default),
    }
}

/// 解析 `usize` 类型环境变量。
pub(crate) fn parse_usize_env(key: &str, default: usize) -> Result<usize> {
    match std::env::var(key) {
        Ok(raw) => raw
            .parse::<usize>()
            .map_err(|_| anyhow!("unsupported integer value: {raw}")),
        Err(_) => Ok(default),
    }
}

/// 解析逗号分隔的字符串列表环境变量。
pub(crate) fn parse_csv_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
