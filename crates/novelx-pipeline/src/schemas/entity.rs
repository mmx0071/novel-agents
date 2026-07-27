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

    /// Reader-facing Chinese H2 (canon on disk stays English via [`Self::required_sections`]).
    fn display_zh_map(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            Self::Character => &[
                ("经历", &["History", "经历", "历史"]),
                ("性格", &["Personality", "性格"]),
                ("核心事件", &["Core events", "Core Events", "核心事件"]),
                ("当前状态", &["Current status", "Current Status", "当前状态", "现状"]),
            ],
            Self::Item => &[
                ("来源", &["Origin", "产出地", "来源"]),
                ("用途", &["Usage", "用途"]),
                ("当前状态", &["Current status", "Current Status", "当前状态", "现状"]),
            ],
            Self::Location => &[
                ("概述", &["Overview", "概述"]),
                ("此地势力", &["Factions", "此地势力", "势力"]),
                ("产出资源", &["Production", "产出资源", "产出"]),
            ],
        }
    }
}

/// Validate full card; returns normalized full markdown.
pub fn validate_entity_card(kind: EntityKind, text: &str) -> Result<String, SchemaError> {
    // Models sometimes wrap a full card in ```markdown after a short stub.
    let text = unwrap_nested_entity_card(text);
    let (mut meta, body) = split_fm(&text);
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
    // Normalize aliases → English canon, then → Chinese for the reader.
    let canon = rewrite_h2_aliases(body.trim(), kind.alias_map());
    rewrite_h2_aliases(&canon, kind.display_zh_map())
}

/// Prefer an inner fenced full card (` ```markdown --- … ``` `) when present.
fn unwrap_nested_entity_card(text: &str) -> String {
    let t = text.trim();
    if let Some(start) = t.find("```") {
        if start > 0 {
            let after = &t[start..];
            let rest = after.strip_prefix("```").unwrap_or(after);
            let rest = rest
                .strip_prefix("markdown")
                .or_else(|| rest.strip_prefix("md"))
                .unwrap_or(rest);
            let rest = rest
                .strip_prefix('\r')
                .unwrap_or(rest)
                .strip_prefix('\n')
                .unwrap_or(rest);
            if let Some(end) = rest.find("```") {
                let inner = rest[..end].trim();
                if inner.contains("---")
                    && (inner.contains("## History")
                        || inner.contains("## 经历")
                        || inner.contains("## Origin")
                        || inner.contains("## 来源")
                        || inner.contains("## Overview")
                        || inner.contains("## 概述"))
                {
                    return inner.to_string();
                }
            }
        }
    }
    t.to_string()
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
    fn display_uses_chinese_headings() {
        let t = r#"---
name: 甲
status: active
---

## History
经历正文。

## Personality
性格正文。

## Core events
- 事件

## Current status
现状正文。
"#;
        let shown = display_entity_card(EntityKind::Character, t);
        assert!(shown.contains("## 经历"), "{shown}");
        assert!(shown.contains("## 性格"), "{shown}");
        assert!(shown.contains("## 核心事件"), "{shown}");
        assert!(shown.contains("## 当前状态"), "{shown}");
        assert!(!shown.contains("## History"), "{shown}");
        assert!(!shown.contains("---"), "{shown}");
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

    #[test]
    fn unwraps_fenced_nested_character_card() {
        let t = r#"---
name: 甲（别名）
status: active
---

# 甲（别名）

一句话 stub。

```markdown
---
name: 甲（别名）
status: exited
aliases: 别名
---

## History
经历。

## Personality
性格。

## Core events
- 事件

## Current status
已故。
```
"#;
        let out = validate_entity_card(EntityKind::Character, t).unwrap();
        assert!(!out.contains("```"), "{out}");
        assert!(out.contains("status: exited") || out.contains("status:exited"), "{out}");
        assert!(out.contains("## History"));
        assert!(!out.contains("一句话 stub"));
    }
}
