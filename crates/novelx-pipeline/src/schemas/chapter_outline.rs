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
    /// Active item cards this chapter (canonical names). Missing → [].
    #[serde(default)]
    pub items: Vec<String>,
    /// Active location cards this chapter (canonical names). Missing → [].
    #[serde(default)]
    pub locations: Vec<String>,
    pub scene_tags: Vec<String>,
    pub cliffhanger: String,
    pub lore_queries: Vec<String>,
}

/// Names declared in the chapter outline for CanonContext card loading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutlineEntityRoster {
    pub characters: Vec<String>,
    pub items: Vec<String>,
    pub locations: Vec<String>,
}

impl ChapterOutline {
    pub fn entity_roster(&self) -> OutlineEntityRoster {
        OutlineEntityRoster {
            characters: self.characters.clone(),
            items: self.items.clone(),
            locations: self.locations.clone(),
        }
    }
}

/// Best-effort roster from outline JSON / text (empty if unparseable).
pub fn outline_entity_roster(outline: &str) -> OutlineEntityRoster {
    parse_chapter_outline_text(outline)
        .map(|o| o.entity_roster())
        .unwrap_or_default()
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
    let softened = soften_json_quotes(&extracted);
    let repaired = repair_model_json(&softened);
    let candidates = [
        extracted.clone(),
        softened.clone(),
        repaired.clone(),
        repair_model_json(&soften_json_quotes(&raw)),
        soften_json_quotes(&raw),
        raw.clone(),
    ];
    let mut last_err = String::new();
    let mut seen = std::collections::HashSet::new();
    for c in &candidates {
        let key = c.trim();
        if key.is_empty() || !seen.insert(key.to_string()) {
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

/// Best-effort repair for common LLM JSON mistakes:
/// trailing commas; bare newlines in strings; unescaped `"` inside string values.
fn repair_model_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut in_str = false;
    let mut escape = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            if escape {
                out.push(c);
                escape = false;
                i += 1;
                continue;
            }
            if c == '\\' {
                out.push(c);
                escape = true;
                i += 1;
                continue;
            }
            if c == '\n' || c == '\r' {
                // Illegal raw newline inside JSON string → escape.
                if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                    i += 1;
                }
                out.push_str("\\n");
                i += 1;
                continue;
            }
            if c == '"' {
                // Closing quote if next non-ws is structural; else escape.
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let next = chars.get(j).copied();
                let closes = matches!(next, Some(',' | '}' | ']' | ':') | None);
                if closes {
                    out.push('"');
                    in_str = false;
                } else {
                    out.push_str("\\\"");
                }
                i += 1;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        // Outside strings.
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        // Trailing comma before } or ].
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if matches!(chars.get(j).copied(), Some('}' | ']')) {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
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
    // items / locations: preferred for card loading; omit → [] (legacy outlines).
    let items = optional_string_array(obj, "items", &mut missing)?;
    let locations = optional_string_array(obj, "locations", &mut missing)?;
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
        items,
        locations,
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
    lines.push("## 本章物品".into());
    if o.items.is_empty() {
        lines.push("- （无）".into());
    } else {
        for c in &o.items {
            lines.push(format!("- {c}"));
        }
    }
    lines.push(String::new());
    lines.push("## 本章地点".into());
    if o.locations.is_empty() {
        lines.push("- （无）".into());
    } else {
        for c in &o.locations {
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
        Some(Value::Array(arr)) => parse_string_array_items(key, arr, missing),
        Some(_) => {
            missing.push(format!("{key}（须为字符串数组）"));
            Ok(vec![])
        }
    }
}

/// Like [`require_string_array`], but missing / null → empty (legacy-friendly).
fn optional_string_array(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    missing: &mut Vec<String>,
) -> Result<Vec<String>, SchemaError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(arr)) => parse_string_array_items(key, arr, missing),
        Some(_) => {
            missing.push(format!("{key}（须为字符串数组）"));
            Ok(vec![])
        }
    }
}

fn parse_string_array_items(
    key: &str,
    arr: &[Value],
    missing: &mut Vec<String>,
) -> Result<Vec<String>, SchemaError> {
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
            "items": ["旧钥"],
            "locations": ["城门"],
            "scene_tags": ["chase"],
            "cliffhanger": "门开了",
            "lore_queries": ["甲伤势"]
        })
    }

    #[test]
    fn accepts_valid() {
        let o = validate_chapter_outline(&sample()).unwrap();
        assert_eq!(o.title, "试章");
        assert_eq!(o.items, vec!["旧钥".to_string()]);
        assert_eq!(o.locations, vec!["城门".to_string()]);
        let md = display_chapter_outline(&o);
        assert!(md.contains("## 关键事件"));
        assert!(md.contains("## 本章物品"));
        assert!(md.contains("旧钥"));
    }

    #[test]
    fn legacy_outline_without_items_locations_ok() {
        let mut v = sample();
        let obj = v.as_object_mut().unwrap();
        obj.remove("items");
        obj.remove("locations");
        let o = validate_chapter_outline(&v).unwrap();
        assert!(o.items.is_empty());
        assert!(o.locations.is_empty());
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

    #[test]
    fn repairs_unescaped_quotes_and_trailing_comma() {
        // Line-shaped like planner output: inner ASCII quotes + trailing comma.
        let text = r#"{
  "title": "试章",
  "pov": "甲",
  "time_location": "夜·城",
  "goal": "到达",
  "conflict": "对方说"别去"并挡住去路",
  "emotion_curve": "紧→松",
  "key_events": ["开场危机", "中段转折"],
  "characters": ["甲"],
  "scene_tags": ["chase"],
  "cliffhanger": "门开了",
  "lore_queries": ["甲伤势"],
}"#;
        let o = parse_chapter_outline_text(text).expect("repaired parse");
        assert!(o.conflict.contains("别去"));
        assert_eq!(o.title, "试章");
    }
}
