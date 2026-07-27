//! Config-driven studio intents (Codex-style thin matcher).
//!
//! Patterns live in `config/intents.yaml`. Rust only loads, scores, and extracts
//! chapter numbers — no scattered Chinese `contains` tables in `lib.rs`.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearHistory {
    Never,
    OnStart,
    OnSuccess,
}

impl Default for ClearHistory {
    fn default() -> Self {
        Self::Never
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    None,
    Chapter,
    ChapterOptional,
    ChapterRange,
    /// Optional `第N卷` → `volume` arg (e.g. list_plots filter).
    VolumeOptional,
}

impl Default for ExtractMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IntentSpec {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub require_project: bool,
    #[serde(default)]
    pub extract: ExtractMode,
    #[serde(default)]
    pub clear_history: ClearHistory,
    #[serde(default)]
    pub any_keywords: Vec<String>,
    #[serde(default)]
    pub none_keywords: Vec<String>,
    /// Each inner list must all be present (OR across groups, AND within a group).
    #[serde(default)]
    pub all_keyword_groups: Vec<Vec<String>>,
    #[serde(default)]
    pub default_instructions: Option<String>,
    #[serde(default)]
    pub instructions_min_chars: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
struct IntentFile {
    #[serde(default)]
    intents: Vec<IntentSpec>,
}

#[derive(Debug, Clone)]
pub struct IntentMatch {
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub clear_history: ClearHistory,
}

#[derive(Debug, Clone)]
pub struct IntentRouter {
    intents: Arc<Vec<IntentSpec>>,
}

impl IntentRouter {
    pub fn load(config_root: &Path) -> Result<Self> {
        let path = config_root.join("intents.yaml");
        if !path.exists() {
            tracing::warn!(path = %path.display(), "intents.yaml missing; deterministic intents disabled");
            return Ok(Self {
                intents: Arc::new(Vec::new()),
            });
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let file: IntentFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        let mut intents = file.intents;
        intents.sort_by(|a, b| b.priority.cmp(&a.priority));
        tracing::info!(count = intents.len(), "studio intents loaded");
        Ok(Self {
            intents: Arc::new(intents),
        })
    }

    pub fn empty() -> Self {
        Self {
            intents: Arc::new(Vec::new()),
        }
    }

    pub fn match_text(&self, text: &str, project: Option<&str>) -> Option<IntentMatch> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        let t_lower = t.to_lowercase();
        for spec in self.intents.iter() {
            if spec.require_project && project.filter(|p| !p.is_empty() && *p != "_").is_none() {
                continue;
            }
            if !keyword_hit(spec, t, &t_lower) {
                continue;
            }
            if let Some(m) = build_match(spec, t, project) {
                return Some(m);
            }
        }
        None
    }
}

fn keyword_hit(spec: &IntentSpec, t: &str, t_lower: &str) -> bool {
    if spec.none_keywords.iter().any(|k| contains_ci(t, t_lower, k)) {
        return false;
    }
    let any_ok = spec.any_keywords.is_empty()
        || spec
            .any_keywords
            .iter()
            .any(|k| contains_ci(t, t_lower, k));
    let groups_ok = spec.all_keyword_groups.is_empty()
        || spec.all_keyword_groups.iter().any(|group| {
            !group.is_empty() && group.iter().all(|k| contains_ci(t, t_lower, k))
        });
    // When both any_keywords and groups are set, either path may qualify (OR).
    if !spec.any_keywords.is_empty() && !spec.all_keyword_groups.is_empty() {
        return (spec
            .any_keywords
            .iter()
            .any(|k| contains_ci(t, t_lower, k)))
            || spec.all_keyword_groups.iter().any(|group| {
                !group.is_empty() && group.iter().all(|k| contains_ci(t, t_lower, k))
            });
    }
    any_ok && groups_ok
}

fn contains_ci(t: &str, t_lower: &str, key: &str) -> bool {
    if key.bytes().all(|b| b.is_ascii()) {
        t_lower.contains(&key.to_lowercase())
    } else {
        t.contains(key)
    }
}

fn build_match(spec: &IntentSpec, t: &str, project: Option<&str>) -> Option<IntentMatch> {
    let project = project.filter(|p| !p.is_empty() && *p != "_")?;
    let mut args = json!({ "project": project });

    match spec.extract {
        ExtractMode::None => {}
        ExtractMode::Chapter => {
            let chapter = parse_chapter_number(t)?;
            args["chapter"] = json!(chapter);
        }
        ExtractMode::ChapterOptional => {
            if let Some(chapter) = parse_chapter_number(t) {
                args["chapter"] = json!(chapter);
            }
        }
        ExtractMode::ChapterRange => {
            let (from, to) = parse_chapter_range(t)?;
            if from == to {
                return None;
            }
            args["from"] = json!(from);
            args["to"] = json!(to);
            args["action"] = json!("start");
        }
        ExtractMode::VolumeOptional => {
            if let Some(volume) = parse_volume_number(t) {
                args["volume"] = json!(volume);
            }
        }
    }

    // Resume an in-progress multi-chapter audit queue.
    if spec.id == "audit_queue_continue" {
        args["action"] = json!("continue");
    }

    if spec.tool == "revise_chapter" || spec.tool == "revise_outline" {
        let min = spec.instructions_min_chars.unwrap_or(24);
        let fallback = if spec.tool == "revise_outline" {
            "按用户要求修订本章章纲。"
        } else {
            "按用户要求修订本章。"
        };
        let instructions = if t.chars().count() >= min {
            t.to_string()
        } else {
            spec.default_instructions
                .clone()
                .unwrap_or_else(|| fallback.into())
        };
        args["instructions"] = json!(instructions);
    }

    Some(IntentMatch {
        id: spec.id.clone(),
        tool: spec.tool.clone(),
        args,
        clear_history: spec.clear_history,
    })
}

pub fn parse_chapter_number(text: &str) -> Option<u32> {
    for n in (1..=99).rev() {
        if text.contains(&format!("第{n}章")) || text.contains(&format!("chapter {n}")) {
            return Some(n);
        }
    }
    const CN: &[&str] = &[
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二", "十三",
        "十四", "十五", "十六", "十七", "十八", "十九", "二十",
    ];
    for (n, s) in CN.iter().enumerate().skip(1) {
        if text.contains(&format!("第{s}章")) {
            return Some(n as u32);
        }
    }
    None
}

pub fn parse_volume_number(text: &str) -> Option<u32> {
    for n in (1..=99).rev() {
        if text.contains(&format!("第{n}卷")) || text.contains(&format!("volume {n}")) {
            return Some(n);
        }
    }
    const CN: &[&str] = &[
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二", "十三",
        "十四", "十五", "十六", "十七", "十八", "十九", "二十",
    ];
    for (n, s) in CN.iter().enumerate().skip(1) {
        if text.contains(&format!("第{s}卷")) {
            return Some(n as u32);
        }
    }
    if text.contains("本卷") || text.contains("这一卷") || text.contains("该卷") {
        return None; // caller may still list all; no numeric filter
    }
    None
}

pub fn parse_chapter_range(text: &str) -> Option<(u32, u32)> {
    // 1-8章 / 1–8章 / 第1-8章 / 第1章到第8章 / 第1章至第8章
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if !chars[i].is_ascii_digit() {
            continue;
        }
        let mut j = i;
        let mut a: u32 = 0;
        while j < chars.len() && chars[j].is_ascii_digit() {
            a = a.saturating_mul(10).saturating_add(chars[j].to_digit(10).unwrap_or(0));
            j += 1;
        }
        if a == 0 {
            continue;
        }
        let sep = chars.get(j).copied();
        if !matches!(sep, Some('-' | '–' | '—' | '~' | '到' | '至')) {
            if sep == Some('章') && j + 1 < chars.len() {
                let mut k = j + 1;
                while k < chars.len()
                    && (chars[k] == '到' || chars[k] == '至' || chars[k].is_whitespace())
                {
                    k += 1;
                }
                if k < chars.len() && chars[k] == '第' {
                    k += 1;
                }
                let mut b: u32 = 0;
                let mut digits = false;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    digits = true;
                    b = b
                        .saturating_mul(10)
                        .saturating_add(chars[k].to_digit(10).unwrap_or(0));
                    k += 1;
                }
                if digits && b >= a {
                    return Some((a.max(1), b));
                }
            }
            continue;
        }
        j += 1;
        if chars.get(j) == Some(&'第') {
            j += 1;
        }
        let mut b: u32 = 0;
        let mut digits = false;
        while j < chars.len() && chars[j].is_ascii_digit() {
            digits = true;
            b = b.saturating_mul(10).saturating_add(chars[j].to_digit(10).unwrap_or(0));
            j += 1;
        }
        if digits && b >= a {
            return Some((a.max(1), b));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router_from_yaml(yaml: &str) -> IntentRouter {
        let file: IntentFile = serde_yaml::from_str(yaml).unwrap();
        let mut intents = file.intents;
        intents.sort_by(|a, b| b.priority.cmp(&a.priority));
        IntentRouter {
            intents: Arc::new(intents),
        }
    }

    #[test]
    fn matches_write_audit_and_range() {
        let yaml = include_str!("../../../config/intents.yaml");
        let r = router_from_yaml(yaml);

        let w = r.match_text("写第10章", Some("demo")).unwrap();
        assert_eq!(w.tool, "continue_writing");
        assert_eq!(w.args["chapter"], 10);
        assert_eq!(w.clear_history, ClearHistory::OnStart);

        let a = r.match_text("检阅第九章", Some("demo")).unwrap();
        assert_eq!(a.tool, "audit_chapter");
        assert_eq!(a.args["chapter"], 9);

        let q = r.match_text("审阅1-8章", Some("demo")).unwrap();
        assert_eq!(q.tool, "audit_chapters");
        assert_eq!(q.args["from"], 1);
        assert_eq!(q.args["to"], 8);

        let rev = r.match_text("修正第五章", Some("demo")).unwrap();
        assert_eq!(rev.tool, "revise_chapter");
        assert_eq!(rev.args["chapter"], 5);

        let outline = r.match_text("修正第五章章纲：加强章末钩子", Some("demo")).unwrap();
        assert_eq!(outline.tool, "revise_outline");
        assert_eq!(outline.args["chapter"], 5);
        assert!(outline.args["instructions"].as_str().unwrap().contains("章纲"));

        let outline2 = r.match_text("改第2章章纲，补地点名单", Some("demo")).unwrap();
        assert_eq!(outline2.tool, "revise_outline");
        assert_eq!(outline2.args["chapter"], 2);

        assert!(r.match_text("今天天气不错", Some("demo")).is_none());
        assert!(r.match_text("写第10章", None).is_none());

        // Composite handoff / setup tasks must not collapse into continue_writing.
        assert!(r
            .match_text(
                "去重设定库后 design_plot，再写第25章",
                Some("demo"),
            )
            .is_none());
        assert!(r
            .match_text("先写卷纲和剧情卡，再写第25章", Some("demo"))
            .is_none());

        let vol = r.match_text("审这一卷", Some("demo")).unwrap();
        assert_eq!(vol.tool, "audit_volume");
        let vol2 = r.match_text("卷末复盘", Some("demo")).unwrap();
        assert_eq!(vol2.tool, "audit_volume");

        let cont = r.match_text("继续审阅", Some("demo")).unwrap();
        assert_eq!(cont.id, "audit_queue_continue");
        assert_eq!(cont.tool, "audit_chapters");
        assert_eq!(cont.args["action"], "continue");

        // Status questions must list plots — never continue_writing.
        let plots = r
            .match_text("对照剧情卡 现在到哪了", Some("demo"))
            .unwrap();
        assert_eq!(plots.id, "plot_status");
        assert_eq!(plots.tool, "list_plots");
        let plots2 = r.match_text("目前的剧情推进到哪了", Some("demo")).unwrap();
        assert_eq!(plots2.id, "plot_status");
        let plots3 = r.match_text("剧情进行到哪了", Some("demo")).unwrap();
        assert_eq!(plots3.id, "plot_status");
        assert_eq!(plots3.tool, "list_plots");
        let plots4 = r.match_text("第三卷进行到哪了", Some("demo")).unwrap();
        assert_eq!(plots4.id, "plot_status");
        assert_eq!(plots4.tool, "list_plots");
        assert_eq!(plots4.args["volume"], 3);
        assert_eq!(parse_volume_number("第三卷进度"), Some(3));
        assert_eq!(parse_volume_number("第3卷到哪了"), Some(3));
    }

    #[test]
    fn parse_chapter_range_variants() {
        assert_eq!(parse_chapter_range("审阅第1-8章"), Some((1, 8)));
        assert_eq!(parse_chapter_range("第1章到第8章审校"), Some((1, 8)));
        assert_eq!(parse_chapter_range("审校第3章"), None);
    }
}
