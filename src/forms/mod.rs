//! 表单模块，负责从本地 Markdown 目录加载表单定义，并校验结构化抽取结果。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Map, Value};

/// 本地表单仓库，按 `form_id -> markdown 文件` 的方式解析表单定义。
#[derive(Debug, Clone)]
pub struct FormCatalog {
    markdown_dir: PathBuf,
}

/// 单个表单的解析结果。
#[derive(Debug, Clone)]
pub struct FormDefinition {
    pub form_id: String,
    pub title: Option<String>,
    pub instructions: Option<String>,
    pub schema: Value,
    pub source_path: PathBuf,
}

/// 单个字段的校验问题。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldValidationIssue {
    pub field: String,
    pub message: String,
}

/// 表单结果校验报告。
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct FormValidationReport {
    pub missing_fields: Vec<String>,
    pub invalid_fields: Vec<FieldValidationIssue>,
    pub warnings: Vec<String>,
}

impl FormCatalog {
    /// 基于本地 Markdown 目录创建表单仓库。
    pub fn new(markdown_dir: PathBuf) -> Self {
        Self { markdown_dir }
    }

    /// 根据 `form_id` 读取并解析对应 Markdown 表单。
    pub fn load(&self, form_id: &str) -> Result<FormDefinition> {
        if !is_valid_form_id(form_id) {
            bail!("invalid form_id: only letters, digits, '-' and '_' are allowed");
        }

        let path = self.markdown_dir.join(format!("{form_id}.md"));
        let markdown = fs::read_to_string(&path)
            .with_context(|| format!("failed to read form markdown: {}", path.display()))?;

        parse_form_markdown(form_id, &path, &markdown)
    }
}

/// 按表单 schema 校验抽取后的 JSON 数据。
pub fn validate_form_data(schema: &Value, data: &Value) -> FormValidationReport {
    let mut report = FormValidationReport::default();
    validate_value(schema, data, "", &mut report);
    report
}

fn parse_form_markdown(
    form_id: &str,
    source_path: &Path,
    markdown: &str,
) -> Result<FormDefinition> {
    let title = extract_title(markdown);
    let instructions = extract_named_section(
        markdown,
        &["instructions", "instruction", "提取说明", "抽取说明"],
    );
    let schema = if let Some(section) = extract_named_section(
        markdown,
        &["schema", "json schema", "表单结构", "表单 schema"],
    ) {
        parse_schema_from_section(&section)?
    } else if let Some(schema_text) = extract_first_json_code_fence(markdown) {
        serde_json::from_str::<Value>(&schema_text)
            .context("failed to parse JSON schema code fence from markdown")?
    } else {
        parse_schema_from_fields_section(markdown)?
    };

    Ok(FormDefinition {
        form_id: form_id.to_string(),
        title,
        instructions,
        schema,
        source_path: source_path.to_path_buf(),
    })
}

fn parse_schema_from_section(section: &str) -> Result<Value> {
    if let Some(schema_text) = extract_first_json_code_fence(section) {
        return serde_json::from_str::<Value>(&schema_text)
            .context("failed to parse JSON schema from schema section");
    }
    Err(anyhow!(
        "schema section exists but no valid JSON code fence was found"
    ))
}

fn parse_schema_from_fields_section(markdown: &str) -> Result<Value> {
    let section = extract_named_section(markdown, &["fields", "field", "字段", "表单字段"])
        .ok_or_else(|| {
            anyhow!("form markdown must contain either a schema section or a fields section")
        })?;
    let table_lines = section
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .map(str::to_string)
        .collect::<Vec<_>>();

    if table_lines.len() < 3 {
        bail!(
            "fields section must contain a markdown table with header, separator and at least one row"
        );
    }

    let headers = parse_markdown_table_row(&table_lines[0]);
    let field_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("field")))
        .ok_or_else(|| anyhow!("fields table must contain a 'field' or '字段名' column"))?;
    let type_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("type")));
    let required_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("required")));
    let enum_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("enum")));
    let description_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("description")));
    let pattern_index = headers
        .iter()
        .position(|header| matches!(canonical_header(header), Some("pattern")));

    let mut properties = Map::new();
    let mut required_fields = Vec::new();

    for line in table_lines.into_iter().skip(1) {
        let cells = parse_markdown_table_row(&line);
        if cells.is_empty() || cells.iter().all(|cell| is_separator_cell(cell)) {
            continue;
        }

        let field_name = cells
            .get(field_index)
            .map(|value| value.trim())
            .unwrap_or("");
        if field_name.is_empty() {
            continue;
        }

        let field_type = type_index
            .and_then(|index| cells.get(index))
            .map(|value| normalize_schema_type(value))
            .unwrap_or("string");
        let required = required_index
            .and_then(|index| cells.get(index))
            .map(|value| parse_required_flag(value))
            .unwrap_or(false);
        let enum_values = enum_index
            .and_then(|index| cells.get(index))
            .and_then(|value| {
                let items = split_enum_values(value);
                if items.is_empty() { None } else { Some(items) }
            });
        let description = description_index
            .and_then(|index| cells.get(index))
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let pattern = pattern_index
            .and_then(|index| cells.get(index))
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());

        let mut property = Map::new();
        property.insert("type".to_string(), Value::String(field_type.to_string()));
        if let Some(description) = description {
            property.insert(
                "description".to_string(),
                Value::String(description.to_string()),
            );
        }
        if let Some(pattern) = pattern {
            property.insert("pattern".to_string(), Value::String(pattern.to_string()));
        }
        if let Some(enum_values) = enum_values {
            property.insert(
                "enum".to_string(),
                Value::Array(enum_values.into_iter().map(Value::String).collect()),
            );
        }

        if required {
            required_fields.push(Value::String(field_name.to_string()));
        }
        properties.insert(field_name.to_string(), Value::Object(property));
    }

    if properties.is_empty() {
        bail!("fields table did not produce any schema properties");
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    schema.insert("additionalProperties".to_string(), Value::Bool(false));
    if !required_fields.is_empty() {
        schema.insert("required".to_string(), Value::Array(required_fields));
    }

    Ok(Value::Object(schema))
}

fn extract_title(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|value| !value.is_empty())
}

fn extract_named_section(markdown: &str, heading_names: &[&str]) -> Option<String> {
    let normalized_names = heading_names
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect::<Vec<_>>();
    let mut collecting = false;
    let mut buffer = Vec::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim().to_lowercase();
            if collecting {
                break;
            }
            if normalized_names.iter().any(|name| name == &heading) {
                collecting = true;
            }
            continue;
        }

        if collecting {
            buffer.push(line);
        }
    }

    let section = buffer.join("\n").trim().to_string();
    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn extract_first_json_code_fence(markdown: &str) -> Option<String> {
    let mut remaining = markdown;
    while let Some(start) = remaining.find("```") {
        let after_start = &remaining[start + 3..];
        let line_end = after_start.find('\n')?;
        let info = after_start[..line_end].trim().to_lowercase();
        let after_info = &after_start[line_end + 1..];
        let fence_end = after_info.find("```")?;
        let body = after_info[..fence_end].trim();
        if (info.is_empty() || info.contains("json")) && serde_json::from_str::<Value>(body).is_ok()
        {
            return Some(body.to_string());
        }
        remaining = &after_info[fence_end + 3..];
    }
    None
}

fn parse_markdown_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn canonical_header(header: &str) -> Option<&'static str> {
    match header.trim().to_lowercase().as_str() {
        "field" | "name" | "key" | "字段" | "字段名" => Some("field"),
        "type" | "类型" => Some("type"),
        "required" | "必填" | "是否必填" => Some("required"),
        "enum" | "枚举" | "可选值" => Some("enum"),
        "description" | "desc" | "说明" | "描述" => Some("description"),
        "pattern" | "regex" | "正则" | "格式" => Some("pattern"),
        _ => None,
    }
}

fn normalize_schema_type(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "integer" | "int" | "整数" => "integer",
        "number" | "float" | "double" | "数字" => "number",
        "boolean" | "bool" | "布尔" => "boolean",
        "array" | "list" | "数组" => "array",
        "object" | "对象" => "object",
        _ => "string",
    }
}

fn parse_required_flag(raw: &str) -> bool {
    matches!(
        raw.trim().to_lowercase().as_str(),
        "true" | "1" | "yes" | "y" | "required" | "是" | "必填"
    )
}

fn split_enum_values(raw: &str) -> Vec<String> {
    raw.split([',', '，', '/', ';', '；'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn is_separator_cell(cell: &str) -> bool {
    let trimmed = cell.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch == ' ')
}

fn is_valid_form_id(form_id: &str) -> bool {
    !form_id.trim().is_empty()
        && form_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn validate_value(schema: &Value, value: &Value, path: &str, report: &mut FormValidationReport) {
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        match expected_type {
            "object" => validate_object(schema, value, path, report),
            "array" => validate_array(schema, value, path, report),
            "string" => validate_string(schema, value, path, report),
            "integer" => validate_integer(value, path, report),
            "number" => validate_number(value, path, report),
            "boolean" => validate_boolean(value, path, report),
            _ => {}
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|candidate| candidate == value) {
            report.invalid_fields.push(FieldValidationIssue {
                field: display_path(path),
                message: format!(
                    "value must be one of {}",
                    allowed
                        .iter()
                        .map(Value::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }
}

fn validate_object(schema: &Value, value: &Value, path: &str, report: &mut FormValidationReport) {
    let Some(object) = value.as_object() else {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be an object".to_string(),
        });
        return;
    };

    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required_fields = schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for required in required_fields {
        let Some(field_name) = required.as_str() else {
            continue;
        };
        match object.get(field_name) {
            Some(value) if !value.is_null() => {}
            _ => report.missing_fields.push(join_path(path, field_name)),
        }
    }

    for (field_name, field_value) in object {
        if let Some(field_schema) = properties.get(field_name) {
            validate_value(
                field_schema,
                field_value,
                &join_path(path, field_name),
                report,
            );
        } else if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            report.warnings.push(format!(
                "field `{}` is not declared in schema and will be ignored by the client",
                join_path(path, field_name)
            ));
        }
    }
}

fn validate_array(schema: &Value, value: &Value, path: &str, report: &mut FormValidationReport) {
    let Some(items) = value.as_array() else {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be an array".to_string(),
        });
        return;
    };

    if let Some(item_schema) = schema.get("items") {
        for (index, item) in items.iter().enumerate() {
            validate_value(
                item_schema,
                item,
                &format!("{}[{}]", display_path(path), index),
                report,
            );
        }
    }
}

fn validate_string(schema: &Value, value: &Value, path: &str, report: &mut FormValidationReport) {
    let Some(text) = value.as_str() else {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be a string".to_string(),
        });
        return;
    };

    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        match matches_simple_pattern(pattern, text) {
            PatternMatch::Matched => {}
            PatternMatch::NotMatched => report.invalid_fields.push(FieldValidationIssue {
                field: display_path(path),
                message: format!("value does not match pattern `{pattern}`"),
            }),
            PatternMatch::Unsupported => report.warnings.push(format!(
                "field `{}` skipped unsupported pattern validation `{pattern}`",
                display_path(path)
            )),
        }
    }
}

fn validate_integer(value: &Value, path: &str, report: &mut FormValidationReport) {
    let valid = value.as_i64().is_some()
        || value.as_u64().is_some()
        || value
            .as_f64()
            .map(|number| number.fract() == 0.0)
            .unwrap_or(false);
    if !valid {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be an integer".to_string(),
        });
    }
}

fn validate_number(value: &Value, path: &str, report: &mut FormValidationReport) {
    if value.as_f64().is_none() && value.as_i64().is_none() && value.as_u64().is_none() {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be a number".to_string(),
        });
    }
}

fn validate_boolean(value: &Value, path: &str, report: &mut FormValidationReport) {
    if value.as_bool().is_none() {
        report.invalid_fields.push(FieldValidationIssue {
            field: display_path(path),
            message: "value must be a boolean".to_string(),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternMatch {
    Matched,
    NotMatched,
    Unsupported,
}

fn matches_simple_pattern(pattern: &str, text: &str) -> PatternMatch {
    match pattern.trim() {
        r"^\d+$" | r"^[0-9]+$" => {
            if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit()) {
                PatternMatch::Matched
            } else {
                PatternMatch::NotMatched
            }
        }
        r"^\d{11}$" | r"^[0-9]{11}$" => {
            if text.len() == 11 && text.chars().all(|ch| ch.is_ascii_digit()) {
                PatternMatch::Matched
            } else {
                PatternMatch::NotMatched
            }
        }
        r"^1\d{10}$" | r"^1[0-9]{10}$" => {
            if text.len() == 11
                && text.starts_with('1')
                && text.chars().all(|ch| ch.is_ascii_digit())
            {
                PatternMatch::Matched
            } else {
                PatternMatch::NotMatched
            }
        }
        _ => PatternMatch::Unsupported,
    }
}

fn join_path(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}.{segment}")
    }
}

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "$".to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_table_form_definition() {
        let markdown = r#"
# 基础档案

## 提取说明
未明确的字段请返回 null。

## 字段
| 字段名 | 类型 | 必填 | 枚举 | 正则 | 描述 |
| --- | --- | --- | --- | --- | --- |
| name | string | 是 |  |  | 姓名 |
| gender | string | 否 | 男,女 |  | 性别 |
| age | integer | 否 |  |  | 年龄 |
| phone | string | 否 |  | ^1\d{10}$ | 手机号 |
"#;

        let definition = parse_form_markdown(
            "basic_profile",
            Path::new("forms/basic_profile.md"),
            markdown,
        )
        .expect("form markdown should parse");

        assert_eq!(definition.title.as_deref(), Some("基础档案"));
        assert_eq!(
            definition.instructions.as_deref(),
            Some("未明确的字段请返回 null。")
        );
        assert_eq!(definition.schema["properties"]["name"]["type"], "string");
        assert_eq!(definition.schema["properties"]["gender"]["enum"][0], "男");
        assert_eq!(definition.schema["required"][0], "name");
    }

    #[test]
    fn validates_missing_and_invalid_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "gender": { "type": "string", "enum": ["男", "女"] },
                "phone": { "type": "string", "pattern": "^1\\d{10}$" }
            },
            "required": ["name", "gender"]
        });
        let data = serde_json::json!({
            "name": "张三",
            "gender": "未知",
            "phone": "123"
        });

        let report = validate_form_data(&schema, &data);

        assert!(report.missing_fields.is_empty());
        assert_eq!(report.invalid_fields.len(), 2);
        assert!(
            report
                .invalid_fields
                .iter()
                .any(|issue| issue.field == "gender")
        );
        assert!(
            report
                .invalid_fields
                .iter()
                .any(|issue| issue.field == "phone")
        );
    }
}
