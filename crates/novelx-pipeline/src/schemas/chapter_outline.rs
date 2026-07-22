//! Chapter outline: `chapters/NNN/outline.json`.

use super::error::SchemaError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChapterOutline {
    pub title: String,
    pub pov: String,
    pub time_location: String,
    pub goal: String,
    pub conflict: String,
    pub emotion_curve: String,
    pub key_events: Vec<String>,
    pub characters: Vec<String>,
    pub scene_tags: Vec<String>,
    pub cliffhanger: String,
    pub lore_queries: Vec<String>,
}

const REQUIRED_STRINGS: &[&str] = &[
    "title",
    "pov",
    "time_location",
    "goal",
    "conflict",
    "emotion_curve",
    "cliffhanger",
];

/// Strip optional ```json fences and parse.
pub fn parse_chapter_outline_text(text: &str) -> Result<ChapterOutline, SchemaError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SchemaError::new(
            "chapter_outline",
            "outline.json",
            "JSON 解析失败：内容为空",
        ));
    }
    let raw = strip_json_fence(trimmed);
    // Models sometimes wrap JSON in prose — take the outermost object if present.
    let extracted = extract_json_object(&raw).unwrap_or_else(|| raw.clone());
    let candidates = [
        extracted.clone(),
        soften_json_quotes(&extracted),
        raw.clone(),
        soften_json_quotes(&raw),
    ];
    let mut last_err = String::new();
    for c in &candidates {
        if c.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(c) {
            Ok(v) => return validate_chapter_outline(&v),
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(SchemaError::new(
        "chapter_outline",
        "outline.json",
        format!("JSON 解析失败：{last_err}"),
    ))
}

/// First top-level `{ ... }` span (brace-balanced), if any.
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Replace curly/smart double quotes that commonly break model JSON.
fn soften_json_quotes(s: &str) -> String {
    s.replace(['\u{201c}', '\u{201d}', '\u{201e}', '\u{00ab}', '\u{00bb}'], "\"")
        .replace(['\u{2018}', '\u{2019}'], "'")
}

pub fn validate_chapter_outline(v: &Value) -> Result<ChapterOutline, SchemaError> {
    let obj = v.as_object().ok_or_else(|| {
        SchemaError::new("chapter_outline", "root", "章纲必须是 JSON 对象")
    })?;

    let mut missing = Vec::new();
    for k in REQUIRED_STRINGS {
        match obj.get(*k) {
            Some(Value::String(s)) if !s.trim().is_empty() => {}
            Some(Value::String(_)) => missing.push(format!("{k}（空字符串）")),
            Some(_) => missing.push(format!("{k}（须为字符串）")),
            None => missing.push((*k).to_string()),
        }
    }

    let key_events = require_string_array(obj, "key_events", &mut missing)?;
    let characters = require_string_array(obj, "characters", &mut missing)?;
    let scene_tags = require_string_array(obj, "scene_tags", &mut missing)?;
    let lore_queries = require_string_array(obj, "lore_queries", &mut missing)?;

    if !missing.is_empty() {
        return Err(SchemaError::new(
            "chapter_outline",
            "fields",
            format!("缺少或非法字段：{}", missing.join("、")),
        ));
    }

    if key_events.len() < 2 {
        return Err(SchemaError::new(
            "chapter_outline",
            "key_events",
            format!("key_events 至少 2 条，当前 {}", key_events.len()),
        ));
    }

    Ok(ChapterOutline {
        title: str_field(obj, "title"),
        pov: str_field(obj, "pov"),
        time_location: str_field(obj, "time_location"),
        goal: str_field(obj, "goal"),
        conflict: str_field(obj, "conflict"),
        emotion_curve: str_field(obj, "emotion_curve"),
        key_events,
        characters,
        scene_tags,
        cliffhanger: str_field(obj, "cliffhanger"),
        lore_queries,
    })
}

/// Stable Markdown preview for the reader (`<pre>`).
pub fn display_chapter_outline(o: &ChapterOutline) -> String {
    let mut lines = vec![
        format!("# {}", o.title),
        String::new(),
        format!("视角：{}", o.pov),
        format!("时空：{}", o.time_location),
        String::new(),
        "## 目标".into(),
        o.goal.clone(),
        String::new(),
        "## 冲突".into(),
        o.conflict.clone(),
        String::new(),
        "## 情绪曲线".into(),
        o.emotion_curve.clone(),
        String::new(),
        "## 关键事件".into(),
    ];
    for (i, e) in o.key_events.iter().enumerate() {
        lines.push(format!("{}. {e}", i + 1));
    }
    lines.push(String::new());
    lines.push("## 出场人物".into());
    if o.characters.is_empty() {
        lines.push("- （无）".into());
    } else {
        for c in &o.characters {
            lines.push(format!("- {c}"));
        }
    }
    lines.push(String::new());
    lines.push("## 场景标签".into());
    if o.scene_tags.is_empty() {
        lines.push("- （无）".into());
    } else {
        for t in &o.scene_tags {
            lines.push(format!("- {t}"));
        }
    }
    lines.push(String::new());
    lines.push("## 章末钩子".into());
    lines.push(o.cliffhanger.clone());
    lines.push(String::new());
    lines.push("## Lore 查询".into());
    if o.lore_queries.is_empty() {
        lines.push("- （无）".into());
    } else {
        for q in &o.lore_queries {
            lines.push(format!("- {q}"));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

fn str_field(obj: &serde_json::Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn require_string_array(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    missing: &mut Vec<String>,
) -> Result<Vec<String>, SchemaError> {
    match obj.get(key) {
        None => {
            missing.push(key.to_string());
            Ok(vec![])
        }
        Some(Value::Array(arr)) => {
            let mut out = Vec::new();
            for (i, item) in arr.iter().enumerate() {
                match item.as_str() {
                    Some(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
                    Some(_) => missing.push(format!("{key}[{i}]（空）")),
                    None => missing.push(format!("{key}[{i}]（须为字符串）")),
                }
            }
            Ok(out)
        }
        Some(_) => {
            missing.push(format!("{key}（须为字符串数组）"));
            Ok(vec![])
        }
    }
}

pub fn strip_json_fence(text: &str) -> String {
    let t = text.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let rest = t.strip_prefix("```").unwrap_or(t);
    let mut lines = rest.lines();
    let first = lines.next().unwrap_or("");
    let body = if first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        lines.collect::<Vec<_>>().join("\n")
    } else {
        format!("{first}\n{}", lines.collect::<Vec<_>>().join("\n"))
    };
    let body = body.trim();
    body.strip_suffix("```").unwrap_or(body).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "title": "试章",
            "pov": "甲",
            "time_location": "夜·城",
            "goal": "到达",
            "conflict": "阻拦",
            "emotion_curve": "紧→松",
            "key_events": ["开场危机", "中段转折"],
            "characters": ["甲"],
            "scene_tags": ["chase"],
            "cliffhanger": "门开了",
            "lore_queries": ["甲伤势"]
        })
    }

    #[test]
    fn accepts_valid() {
        let o = validate_chapter_outline(&sample()).unwrap();
        assert_eq!(o.title, "试章");
        assert!(display_chapter_outline(&o).contains("## 关键事件"));
    }

    #[test]
    fn rejects_missing_title() {
        let mut v = sample();
        v.as_object_mut().unwrap().remove("title");
        let e = validate_chapter_outline(&v).unwrap_err();
        assert!(e.message.contains("title"));
    }

    #[test]
    fn rejects_short_key_events() {
        let mut v = sample();
        v["key_events"] = json!(["only one"]);
        let e = validate_chapter_outline(&v).unwrap_err();
        assert!(e.message.contains("至少 2"));
    }

    #[test]
    fn parses_fenced_json() {
        let text = "```json\n{\"title\":\"A\",\"pov\":\"B\",\"time_location\":\"C\",\"goal\":\"D\",\"conflict\":\"E\",\"emotion_curve\":\"F\",\"key_events\":[\"1\",\"2\"],\"characters\":[],\"scene_tags\":[],\"cliffhanger\":\"G\",\"lore_queries\":[]}\n```";
        let o = parse_chapter_outline_text(text).unwrap();
        assert_eq!(o.title, "A");
    }

    #[test]
    fn parses_json_wrapped_in_prose() {
        let text = "以下为章纲：\n{\"title\":\"A\",\"pov\":\"B\",\"time_location\":\"C\",\"goal\":\"D\",\"conflict\":\"E\",\"emotion_curve\":\"F\",\"key_events\":[\"1\",\"2\"],\"characters\":[],\"scene_tags\":[],\"cliffhanger\":\"G\",\"lore_queries\":[]}\n请查收。";
        let o = parse_chapter_outline_text(text).unwrap();
        assert_eq!(o.title, "A");
    }

    #[test]
    fn rejects_empty() {
        let e = parse_chapter_outline_text("   ").unwrap_err();
        assert!(e.message.contains("空"));
    }
}
