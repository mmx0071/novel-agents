//! Chapter outline: `chapters/NNN/outline.json`.

use super::error::SchemaError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Max scene-level beats in one chapter outline (budget ≈ 5000–6000 words).
pub const MAX_KEY_EVENTS: usize = 4;

/// Soft cap on a single include / key_event string (chars). Overlong → repair hint.
pub const MAX_OUTLINE_BEAT_CHARS: usize = 48;

/// How strictly to enforce chapter length budget on outline JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineValidateMode {
    /// Read / legacy round-trip: allow `key_events` above [`MAX_KEY_EVENTS`].
    Compatible,
    /// New planner / revise_outline output: enforce event budget.
    EnforceBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChapterOutline {
    pub title: String,
    pub pov: String,
    pub time_location: String,
    pub goal: String,
    pub conflict: String,
    pub emotion_curve: String,
    /// Plot beats this chapter must cover (short; budget for 5k–6k). Missing → [].
    #[serde(default)]
    pub plot_includes: Vec<String>,
    /// Plot beats explicitly deferred to later chapters. Missing → [].
    #[serde(default)]
    pub plot_defers: Vec<String>,
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

/// Soft budget gaps for a follow-up planner repair (empty = OK).
pub fn outline_budget_repair_hint(o: &ChapterOutline, has_active_plot: bool) -> Option<String> {
    let mut parts = Vec::new();
    if o.key_events.len() > MAX_KEY_EVENTS {
        parts.push(format!(
            "key_events 至多 {MAX_KEY_EVENTS} 条（单章篇幅预算），当前 {}",
            o.key_events.len()
        ));
    }
    if o.plot_includes.is_empty() {
        parts.push(
            "plot_includes 至少 1 条：写明本章必须推进的短句（估可写成 5000–6000 字）".into(),
        );
    }
    if has_active_plot && o.plot_defers.is_empty() {
        parts.push(
            "存在进行中剧情卡时 plot_defers 至少 1 条：写明本章不写/不兑现的走向点（含默认不兑现收束条件）"
                .into(),
        );
    }
    let dense_includes: Vec<usize> = o
        .plot_includes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.chars().count() > MAX_OUTLINE_BEAT_CHARS)
        .map(|(i, _)| i + 1)
        .collect();
    if !dense_includes.is_empty() {
        parts.push(format!(
            "plot_includes 第{}条过长（单条宜≤{MAX_OUTLINE_BEAT_CHARS}字短句锚点）；拆条或删细节，多余推进挪入 plot_defers",
            dense_includes
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    let dense_events: Vec<usize> = o
        .key_events
        .iter()
        .enumerate()
        .filter(|(_, s)| s.chars().count() > MAX_OUTLINE_BEAT_CHARS)
        .map(|(i, _)| i + 1)
        .collect();
    if !dense_events.is_empty() {
        parts.push(format!(
            "key_events 第{}条过长（单条宜≤{MAX_OUTLINE_BEAT_CHARS}字场面锚点）；拆成短句或把次要节拍挪入 plot_defers，总数仍≤{MAX_KEY_EVENTS}",
            dense_events
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("；"))
    }
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

/// Strip optional ```json fences and parse (legacy-compatible: `key_events` may exceed 4).
pub fn parse_chapter_outline_text(text: &str) -> Result<ChapterOutline, SchemaError> {
    parse_chapter_outline_text_with(text, OutlineValidateMode::Compatible)
}

/// Parse and enforce chapter budget (`key_events` ≤ [`MAX_KEY_EVENTS`]).
pub fn parse_chapter_outline_text_budget(text: &str) -> Result<ChapterOutline, SchemaError> {
    parse_chapter_outline_text_with(text, OutlineValidateMode::EnforceBudget)
}

fn parse_chapter_outline_text_with(
    text: &str,
    mode: OutlineValidateMode,
) -> Result<ChapterOutline, SchemaError> {
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
            Ok(v) => return validate_chapter_outline_with(&v, mode),
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

/// Validate outline JSON (legacy-compatible: does not reject oversized `key_events`).
pub fn validate_chapter_outline(v: &Value) -> Result<ChapterOutline, SchemaError> {
    validate_chapter_outline_with(v, OutlineValidateMode::Compatible)
}

/// Validate and enforce chapter budget (`key_events` ≤ [`MAX_KEY_EVENTS`]).
pub fn validate_chapter_outline_budget(v: &Value) -> Result<ChapterOutline, SchemaError> {
    validate_chapter_outline_with(v, OutlineValidateMode::EnforceBudget)
}

pub fn validate_chapter_outline_with(
    v: &Value,
    mode: OutlineValidateMode,
) -> Result<ChapterOutline, SchemaError> {
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
    // plot_includes / plot_defers / items / locations: omit → [] (legacy outlines).
    let plot_includes = optional_string_array(obj, "plot_includes", &mut missing)?;
    let plot_defers = optional_string_array(obj, "plot_defers", &mut missing)?;
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
    if mode == OutlineValidateMode::EnforceBudget && key_events.len() > MAX_KEY_EVENTS {
        return Err(SchemaError::new(
            "chapter_outline",
            "key_events",
            format!(
                "key_events 至多 {MAX_KEY_EVENTS} 条（单章篇幅预算），当前 {}",
                key_events.len()
            ),
        ));
    } else if mode == OutlineValidateMode::Compatible && key_events.len() > MAX_KEY_EVENTS {
        tracing::debug!(
            n = key_events.len(),
            max = MAX_KEY_EVENTS,
            "legacy outline key_events above budget; accepted in Compatible mode"
        );
    }

    Ok(ChapterOutline {
        title: str_field(obj, "title"),
        pov: str_field(obj, "pov"),
        time_location: str_field(obj, "time_location"),
        goal: str_field(obj, "goal"),
        conflict: str_field(obj, "conflict"),
        emotion_curve: str_field(obj, "emotion_curve"),
        plot_includes,
        plot_defers,
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
        "## 本章纳入".into(),
    ];
    if o.plot_includes.is_empty() {
        lines.push("- （未标注）".into());
    } else {
        for e in &o.plot_includes {
            lines.push(format!("- {e}"));
        }
    }
    lines.push(String::new());
    lines.push("## 顺延后章".into());
    if o.plot_defers.is_empty() {
        lines.push("- （未标注）".into());
    } else {
        for e in &o.plot_defers {
            lines.push(format!("- {e}"));
        }
    }
    lines.push(String::new());
    lines.push("## 关键事件".into());
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
            "plot_includes": ["抵达城门并受阻"],
            "plot_defers": ["本章不兑现主线收束落点"],
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
        assert_eq!(o.plot_includes, vec!["抵达城门并受阻".to_string()]);
        assert_eq!(o.plot_defers.len(), 1);
        let md = display_chapter_outline(&o);
        assert!(md.contains("## 关键事件"));
        assert!(md.contains("## 本章纳入"));
        assert!(md.contains("## 顺延后章"));
        assert!(md.contains("## 本章物品"));
        assert!(md.contains("旧钥"));
    }

    #[test]
    fn legacy_outline_without_items_locations_ok() {
        let mut v = sample();
        let obj = v.as_object_mut().unwrap();
        obj.remove("items");
        obj.remove("locations");
        obj.remove("plot_includes");
        obj.remove("plot_defers");
        let o = validate_chapter_outline(&v).unwrap();
        assert!(o.items.is_empty());
        assert!(o.locations.is_empty());
        assert!(o.plot_includes.is_empty());
        assert!(o.plot_defers.is_empty());
    }

    #[test]
    fn rejects_too_many_key_events_in_budget_mode() {
        let mut v = sample();
        v["key_events"] = json!(["a", "b", "c", "d", "e"]);
        assert!(validate_chapter_outline(&v).is_ok(), "Compatible accepts legacy");
        let e = validate_chapter_outline_budget(&v).unwrap_err();
        assert!(e.message.contains("至多"), "got: {}", e.message);
    }

    #[test]
    fn budget_repair_hint_covers_includes_and_defers() {
        let mut o = validate_chapter_outline(&sample()).unwrap();
        o.plot_includes.clear();
        o.plot_defers.clear();
        let hint = outline_budget_repair_hint(&o, true).unwrap();
        assert!(hint.contains("plot_includes"));
        assert!(hint.contains("plot_defers"));
        assert!(outline_budget_repair_hint(&o, false).unwrap().contains("plot_includes"));
    }

    #[test]
    fn budget_repair_hint_flags_overlong_single_beat() {
        let mut o = validate_chapter_outline(&sample()).unwrap();
        o.plot_includes = vec!["短推进".into()];
        o.plot_defers = vec!["顺延收束".into()];
        o.key_events = vec![
            "短事件一".into(),
            "这是一条故意写得很长的关键事件描述，把沟通、决策、挂断电话与多件道具揭示全部塞进一句里，明显超过四十八字限制".into(),
        ];
        let hint = outline_budget_repair_hint(&o, true).unwrap();
        assert!(hint.contains("key_events"), "{hint}");
        assert!(hint.contains("过长") || hint.contains(&MAX_OUTLINE_BEAT_CHARS.to_string()), "{hint}");
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
