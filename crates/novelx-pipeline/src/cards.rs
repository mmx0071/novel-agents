//! Markdown setting cards (entities / plots) with simple YAML frontmatter.

use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Soft threshold: sync stubs / thin cards below this are treated incomplete.
const STUB_BODY_CHARS: usize = 400;

/// Normalize entity lifecycle status for frontmatter.
/// Active: active / background / 在场 / empty.
/// Inactive: retired / exited / consumed / 退场 / 已退场 / 消耗 …
pub fn normalize_entity_status(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        return "active".into();
    }
    if matches!(
        s.as_str(),
        "retired"
            | "exited"
            | "exit"
            | "consumed"
            | "expended"
            | "destroyed"
            | "gone"
            | "退场"
            | "已退场"
            | "暂退场"
            | "消耗"
            | "已消耗"
            | "毁坏"
            | "销毁"
    ) || s.contains("退场")
        || s.contains("消耗")
        || s.contains("毁")
    {
        if s.contains("消耗") || matches!(s.as_str(), "consumed" | "expended") {
            return "consumed".into();
        }
        return "exited".into();
    }
    if matches!(s.as_str(), "background" | "背景" | "幕后") {
        return "background".into();
    }
    "active".into()
}

pub fn entity_status_is_active(status: &str) -> bool {
    matches!(
        normalize_entity_status(status).as_str(),
        "active" | "background"
    )
}

/// Read current-volume 卷纲 excerpt for plot design (None if missing/empty).
pub fn read_arc_outline_excerpt(project_dir: &Path, max_chars: usize) -> Option<String> {
    let vol = crate::volume::resolve_arc_outline_volume(project_dir, None);
    let text = crate::volume::read_arc_outline_text(project_dir, vol)?;
    Some(truncate_chars(text.trim(), max_chars))
}

#[derive(Debug, Clone)]
pub struct MarkdownCard {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub title: String,
    pub category: String,
    pub markdown: String,
    pub complete: bool,
    pub meta: HashMap<String, String>,
}

impl MarkdownCard {
    pub fn to_preview_json(&self) -> serde_json::Value {
        let mut obj = json!({
            "id": self.id,
            "slug": self.slug,
            "name": self.name,
            "title": self.title,
            "category": self.category,
            "markdown": self.markdown,
            "complete": self.complete,
            "gaps": [],
        });
        for key in [
            "scope",
            "arc",
            "plot_type",
            "status",
            "holdings",
            "needs_bridge",
            "next_plot",
            "volume_index",
            "arc_index",
        ] {
            if let Some(v) = self.meta.get(key) {
                obj[key] = json!(v);
            }
        }
        obj
    }

    pub fn match_keys(&self) -> Vec<String> {
        let mut keys = vec![self.name.clone(), self.title.clone(), self.slug.clone()];
        if let Some(aliases) = self.meta.get("aliases") {
            for a in aliases.split(|c| c == ',' || c == ';' || c == '|') {
                let t = a.trim();
                if !t.is_empty() {
                    keys.push(t.to_string());
                }
            }
        }
        keys.retain(|k| !k.is_empty());
        keys
    }

    /// Lifecycle from frontmatter `status`. Retired/exited/consumed cards stay out of CanonContext
    /// (unless always-include / protagonist overrides at call site).
    pub fn is_active_for_canon(&self) -> bool {
        entity_status_is_active(self.meta.get("status").map(|s| s.as_str()).unwrap_or(""))
    }

    pub fn chapter_range(&self) -> Option<(u32, u32)> {
        let from = self.meta.get("chapter_from")?.parse().ok()?;
        let to = self
            .meta
            .get("chapter_to")
            .and_then(|s| s.parse().ok())
            .unwrap_or(from);
        Some((from, to))
    }
}

/// Atomic identity tokens for entity names like `严国栋（老严）` / `老严（严国栋）`.
/// Full composite strings are excluded — only parts between / around parentheses.
pub fn entity_identity_parts(name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let norm = name
        .trim()
        .replace('(', "（")
        .replace(')', "）");
    if norm.is_empty() {
        return out;
    }
    let mut buf = String::new();
    for ch in norm.chars() {
        if matches!(ch, '（' | '）' | '、' | '/' | '|' | ';' | '；' | ',') {
            let t = buf.trim().to_string();
            if !t.is_empty() {
                out.insert(t);
            }
            buf.clear();
        } else {
            buf.push(ch);
        }
    }
    let t = buf.trim().to_string();
    if !t.is_empty() {
        out.insert(t);
    }
    out
}

/// True when two display names refer to the same entity (paren alias swap / alias subset).
pub fn entity_names_equivalent(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let na = a.replace('(', "（").replace(')', "）");
    let nb = b.replace('(', "（").replace(')', "）");
    if na == nb {
        return true;
    }
    let ta = entity_identity_parts(&na);
    let tb = entity_identity_parts(&nb);
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    if ta.len() >= 2 && tb.len() >= 2 && ta == tb {
        return true;
    }
    // Bare alias vs `正式名（别名）`
    if ta.len() == 1 && tb.len() >= 2 && tb.contains(ta.iter().next().unwrap()) {
        return true;
    }
    if tb.len() == 1 && ta.len() >= 2 && ta.contains(tb.iter().next().unwrap()) {
        return true;
    }
    false
}

/// Resolve write path for an entity: reuse an existing card when names/aliases match.
/// Returns `(path, existing_card)`.
pub fn resolve_entity_card_path(
    folder: &Path,
    kind: &str,
    requested_name: &str,
) -> (PathBuf, Option<MarkdownCard>) {
    let requested = requested_name.trim();
    let default = folder.join(format!("{requested}.md"));
    if requested.is_empty() {
        return (default, None);
    }
    let cards = load_markdown_cards(folder, kind);
    let mut best: Option<MarkdownCard> = None;
    for c in cards {
        let keys = c.match_keys();
        let hit = keys.iter().any(|k| entity_names_equivalent(k, requested))
            || entity_names_equivalent(&c.name, requested)
            || entity_names_equivalent(&c.slug, requested);
        if !hit {
            continue;
        }
        best = Some(match best {
            None => c,
            Some(prev) => {
                // Prefer the richer / more complete card when several match.
                if c.complete && !prev.complete {
                    c
                } else if c.markdown.len() > prev.markdown.len() {
                    c
                } else {
                    prev
                }
            }
        });
    }
    if let Some(card) = best {
        let path = folder.join(format!("{}.md", card.slug));
        return (path, Some(card));
    }
    (default, None)
}

/// Load `*.md` cards from a directory.
pub fn load_markdown_cards(dir: &Path, kind: &str) -> Vec<MarkdownCard> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let slug = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();
        let (meta, body) = split_simple_frontmatter(&text);
        let name = meta
            .get("name")
            .cloned()
            .or_else(|| meta.get("title").cloned())
            .unwrap_or_else(|| slug.clone());
        let id = meta.get("id").cloned().unwrap_or_else(|| slug.clone());
        let complete = card_is_complete(&meta, &body);
        out.push(MarkdownCard {
            id,
            slug,
            name: name.clone(),
            title: meta.get("title").cloned().unwrap_or(name),
            category: meta
                .get("category")
                .cloned()
                .unwrap_or_else(|| kind.into()),
            markdown: if body.trim().is_empty() {
                text
            } else {
                body
            },
            complete,
            meta,
        });
    }
    out
}

pub fn split_simple_frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let mut meta = HashMap::new();
    let trimmed = text.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (meta, text.to_string());
    }
    let rest = &trimmed[3..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        return (meta, text.to_string());
    };
    let yaml = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim().to_string();
        let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
        if !key.is_empty() {
            meta.insert(key, val);
        }
    }
    (meta, body)
}

fn card_is_complete(meta: &HashMap<String, String>, body: &str) -> bool {
    if let Some(v) = meta.get("complete") {
        return matches!(v.as_str(), "true" | "yes" | "1");
    }
    if meta
        .get("source")
        .map(|s| s == "volume_sync")
        .unwrap_or(false)
    {
        return false;
    }
    let chars = body.chars().count();
    if (body.contains("## 卷末同步摘要") || body.contains("## 剧情同步摘要"))
        && chars < STUB_BODY_CHARS
    {
        return false;
    }
    chars > 80
}

/// Heuristic setting gaps for Web「设定缺口」(soft; never blocks writing).
pub fn collect_entity_gaps(project_dir: &Path) -> Vec<String> {
    let mut gaps = Vec::new();
    let mut all_names: Vec<(String, String)> = Vec::new(); // (group, name)

    for group in ["characters", "items", "locations"] {
        let cards = load_markdown_cards(&project_dir.join("entities").join(group), group);
        for c in &cards {
            all_names.push((group.to_string(), c.name.clone()));
            if !c.complete {
                gaps.push(format!(
                    "[{group}]「{}」待补全（短摘要/同步 stub 或 complete:false）",
                    c.name
                ));
            } else if (c.markdown.contains("## 卷末同步摘要")
                || c.markdown.contains("## 剧情同步摘要"))
                && c.markdown.chars().count() < STUB_BODY_CHARS
            {
                gaps.push(format!(
                    "[{group}]「{}」仍以同步摘要为主，建议补全设定卡",
                    c.name
                ));
            }
        }
    }

    // Near-duplicate filenames / display names (paren alias swap / prefix).
    for i in 0..all_names.len() {
        for j in (i + 1)..all_names.len() {
            let (g1, n1) = &all_names[i];
            let (g2, n2) = &all_names[j];
            if g1 != g2 {
                continue;
            }
            if n1 == n2 || entity_names_equivalent(n1, n2) {
                gaps.push(format!(
                    "[{g1}] 疑似重复：「{n1}」与「{n2}」，建议合并后 delete_entity"
                ));
                continue;
            }
            let (a, b) = if n1.chars().count() <= n2.chars().count() {
                (n1.as_str(), n2.as_str())
            } else {
                (n2.as_str(), n1.as_str())
            };
            if b.starts_with(a) {
                if let Some(next) = b.chars().nth(a.chars().count()) {
                    if next == '（' || next == '(' {
                        gaps.push(format!(
                            "[{g1}] 疑似重复：「{n1}」与「{n2}」，建议合并后 delete_entity"
                        ));
                    }
                }
            }
        }
    }

    // Soft: nomenclature locations without a location card.
    let nom_path = project_dir.join("lore/nomenclature.json");
    if let Ok(text) = std::fs::read_to_string(&nom_path) {
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) {
            let loc_names: Vec<String> = all_names
                .iter()
                .filter(|(g, _)| g == "locations")
                .map(|(_, n)| n.clone())
                .collect();
            if let Some(arr) = root.get("entities").and_then(|x| x.as_array()) {
                for e in arr {
                    let cat = e
                        .get("category")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if cat != "地点" && cat != "location" && cat != "place" {
                        continue;
                    }
                    let name = e
                        .get("canonical_name")
                        .or_else(|| e.get("name"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    if name.is_empty() {
                        continue;
                    }
                    let has = loc_names.iter().any(|n| n == name || n.contains(name) || name.contains(n.as_str()));
                    if !has {
                        gaps.push(format!(
                            "[locations] 名词表有地点「{name}」但无对应地点卡（软提示）"
                        ));
                    }
                }
            }
        }
    }

    gaps.sort();
    gaps.dedup();
    gaps
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Keep the **tail** (recent content) when truncating — for rolling memory.
pub fn truncate_chars_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let skip = count.saturating_sub(max.saturating_sub(1));
    let mut out = String::from("…");
    out.extend(s.chars().skip(skip));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_entity_status_maps_lifecycle() {
        assert_eq!(normalize_entity_status(""), "active");
        assert_eq!(normalize_entity_status("active"), "active");
        assert_eq!(normalize_entity_status("退场"), "exited");
        assert_eq!(normalize_entity_status("retired"), "exited");
        assert_eq!(normalize_entity_status("已消耗"), "consumed");
        assert!(entity_status_is_active("background"));
        assert!(!entity_status_is_active("exited"));
    }

    #[test]
    fn paren_alias_names_are_equivalent() {
        assert!(entity_names_equivalent("严国栋（老严）", "老严（严国栋）"));
        assert!(entity_names_equivalent("老严", "严国栋（老严）"));
        assert!(!entity_names_equivalent("周荣", "严国栋（老严）"));
    }

    #[test]
    fn read_arc_outline_excerpt_requires_content() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-arc-excerpt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("artifacts")).unwrap();
        assert!(read_arc_outline_excerpt(&dir, 100).is_none());
        std::fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# 卷纲\n\n开卷状态：主角抵达边境哨站。\n终止：取得通行印信。\n",
        )
        .unwrap();
        let ex = read_arc_outline_excerpt(&dir, 80).unwrap();
        assert!(ex.contains("哨站"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
