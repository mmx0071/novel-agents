//! Entity cards: `entities/{characters,items,locations}/*.md`.

use super::error::SchemaError;
use super::md::{has_h2, join_fm, rewrite_h2_aliases, split_fm};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Character,
    Item,
    Location,
}

impl EntityKind {
    pub fn from_group(group: &str) -> Option<Self> {
        match group {
            "characters" | "character" => Some(Self::Character),
            "items" | "item" => Some(Self::Item),
            "locations" | "location" => Some(Self::Location),
            _ => None,
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Character => "characters",
            Self::Item => "items",
            Self::Location => "locations",
        }
    }

    fn required_sections(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            Self::Character => &[
                ("History", &["History", "经历", "历史"]),
                ("Personality", &["Personality", "性格"]),
                ("Core events", &["Core events", "Core Events", "核心事件"]),
                ("Current status", &["Current status", "Current Status", "当前状态", "现状"]),
            ],
            Self::Item => &[
                ("Origin", &["Origin", "产出地", "来源"]),
                ("Usage", &["Usage", "用途"]),
                ("Current status", &["Current status", "Current Status", "当前状态", "现状"]),
            ],
            Self::Location => &[
                ("Overview", &["Overview", "概述"]),
                ("Factions", &["Factions", "此地势力", "势力"]),
                ("Production", &["Production", "产出资源", "产出"]),
            ],
        }
    }

    fn alias_map(self) -> &'static [(&'static str, &'static [&'static str])] {
        self.required_sections()
    }
}

/// Validate full card; returns normalized full markdown.
pub fn validate_entity_card(kind: EntityKind, text: &str) -> Result<String, SchemaError> {
    let (mut meta, body) = split_fm(text);
    if meta.is_empty() && !text.trim_start().starts_with("---") {
        return Err(SchemaError::new(
            "entity",
            "frontmatter",
            "设定卡须含 YAML frontmatter（--- … ---）",
        ));
    }
    let name = meta
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if name.is_none() {
        return Err(SchemaError::new(
            "entity",
            "name",
            "frontmatter 缺少 name",
        ));
    }
    let status = meta
        .get("status")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if status.is_none() {
        meta.insert("status".into(), "active".into());
    } else {
        let s = status.unwrap();
        let ok = matches!(
            s.as_str(),
            "active" | "background" | "exited" | "consumed" | "Active" | "Background"
        );
        if !ok {
            return Err(SchemaError::new(
                "entity",
                "status",
                format!("status 非法：{s}（须为 active|background|exited|consumed）"),
            ));
        }
    }

    let mut missing = Vec::new();
    for (canon, aliases) in kind.required_sections() {
        if !has_h2(&body, aliases) {
            missing.push(format!("## {canon}"));
        }
    }
    if !missing.is_empty() {
        return Err(SchemaError::new(
            "entity",
            "sections",
            format!(
                "{} 卡缺少必填节：{}",
                kind.group(),
                missing.join("、")
            ),
        ));
    }

    let norm_body = rewrite_h2_aliases(body.trim(), kind.alias_map());
    Ok(join_fm(&meta, &norm_body))
}

pub fn display_entity_card(kind: EntityKind, text: &str) -> String {
    let (_meta, body) = split_fm(text);
    rewrite_h2_aliases(body.trim(), kind.alias_map())
}

pub fn validate_entity_body_edit(
    kind: EntityKind,
    existing_full: &str,
    new_body: &str,
) -> Result<String, SchemaError> {
    let (meta, _) = split_fm(existing_full);
    let synthetic = join_fm(&meta, new_body);
    let full = validate_entity_card(kind, &synthetic)?;
    let (_m, body) = split_fm(&full);
    Ok(body)
}

/// Minimal legal skeleton for migration (no invented plot).
pub fn minimal_entity_skeleton(kind: EntityKind, name: &str, existing_body: &str) -> String {
    let (mut meta, body) = split_fm(existing_body);
    if !meta.contains_key("name") {
        meta.insert("name".into(), name.into());
    }
    if !meta.contains_key("status") {
        meta.insert("status".into(), "active".into());
    }
    let mut body = body;
    for (canon, aliases) in kind.required_sections() {
        if !has_h2(&body, aliases) {
            body = format!("{}\n\n## {canon}\n\n（待补全）\n", body.trim_end());
        }
    }
    join_fm(&meta, &rewrite_h2_aliases(body.trim(), kind.alias_map()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_character() {
        let t = r#"---
name: 甲
status: active
---

## History
经历。

## Personality
性格。

## Core events
- 事件

## Current status
现状。
"#;
        assert!(validate_entity_card(EntityKind::Character, t).is_ok());
    }

    #[test]
    fn rejects_missing_personality() {
        let t = r#"---
name: 甲
status: active
---

## History
x

## Core events
y

## Current status
z
"#;
        let e = validate_entity_card(EntityKind::Character, t).unwrap_err();
        assert!(e.message.contains("Personality"));
    }

    #[test]
    fn chinese_aliases() {
        let t = r#"---
name: 刀
status: active
---

## 来源
山。

## 用途
砍。

## 当前状态
在用。
"#;
        let out = validate_entity_card(EntityKind::Item, t).unwrap();
        assert!(out.contains("## Origin"));
    }
}
