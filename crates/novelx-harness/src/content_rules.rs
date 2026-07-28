//! Deterministic draft hard rules — engine in Rust, data in `config/content_rules.yaml`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRuleViolation {
    pub rule: String,
    pub message: String,
    /// From `rules.<id>.blocking`. False = report-only (does not block publish).
    #[serde(default = "default_true")]
    pub blocking: bool,
}

impl Default for ContentRuleViolation {
    fn default() -> Self {
        Self {
            rule: String::new(),
            message: String::new(),
            blocking: true,
        }
    }
}

/// True when any violation is configured as blocking.
pub fn has_blocking_violation(violations: &[ContentRuleViolation]) -> bool {
    violations.iter().any(|v| v.blocking)
}

fn violation(rule: &str, message: String, blocking: bool) -> ContentRuleViolation {
    ContentRuleViolation {
        rule: rule.into(),
        message,
        blocking,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleMeta {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub blocking: bool,
}

fn default_true() -> bool {
    true
}

impl Default for RuleMeta {
    fn default() -> Self {
        Self {
            enabled: true,
            title: String::new(),
            description: String::new(),
            blocking: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BannedNameRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_contradiction_marker")]
    pub marker: String,
    #[serde(default = "default_contradiction_msg")]
    pub message: String,
}

fn default_contradiction_marker() -> String {
    "[CONTRADICTION]".into()
}
fn default_contradiction_msg() -> String {
    "正文残留管道内部标记：{marker}".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TooManyNewFactsRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_max_new_facts")]
    pub max_count: usize,
    #[serde(default = "default_new_fact_pattern")]
    pub pattern: String,
    #[serde(default = "default_new_facts_msg")]
    pub message: String,
}

fn default_max_new_facts() -> usize {
    8
}
fn default_new_fact_pattern() -> String {
    r"\[NEW_FACT:[^\]]+\]".into()
}
fn default_new_facts_msg() -> String {
    "[NEW_FACT] 过多（{count}），需收敛".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaChapterRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_meta_pattern")]
    pub pattern: String,
    #[serde(default = "default_true")]
    pub allow_heading: bool,
    #[serde(default = "default_rewrite_replacement")]
    pub rewrite_replacement: String,
    #[serde(default = "default_meta_msg")]
    pub message: String,
}

fn default_meta_pattern() -> String {
    r"第\s*[一二三四五六七八九十百千零〇两\d]+\s*章".into()
}
fn default_rewrite_replacement() -> String {
    "此前".into()
}
fn default_meta_msg() -> String {
    "正文出现章号元叙述「{match}」（约第{line}行）：角色不应知道「第N章」；改用故事内时间/事件指称（如「上次会面时」「那次事故之后」）".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineMetaLeakRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_pipeline_meta_patterns")]
    pub patterns: Vec<String>,
    #[serde(default = "default_pipeline_meta_msg")]
    pub message: String,
}

fn default_pipeline_meta_patterns() -> Vec<String> {
    [
        "章纲里",
        "章纲写",
        "章纲中",
        "按大纲",
        "大纲里",
        "大纲写",
        "设定上",
        "设定文档",
        "写作说明",
        "按章纲",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_pipeline_meta_msg() -> String {
    "正文出现管线元叙述「{match}」（约第{line}行）：禁止章纲/大纲/设定对读；写前自行对齐后只写场面"
        .into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountdownJumpRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_countdown_recall")]
    pub recall_markers: Vec<String>,
    #[serde(default = "default_countdown_msg")]
    pub message: String,
}

fn default_countdown_recall() -> Vec<String> {
    [
        "最初", "原先", "开始时", "开始时是", "曾经", "那时", "当时还", "刚才还是",
        "刚才还剩", "想起", "回忆", "梦里", "梦中", "总额", "一共", "总共", "原先是",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
fn default_countdown_msg() -> String {
    "章内倒计时/剩余时长回跳：约第{prev_line}行「还剩/剩余 ≈ {prev_secs}秒」之后，约第{line}行变成更大读数「≈ {secs}秒」。\
     当前读数须单调递减；回忆初始值须标明「最初/原先」。依据：…{prev_snippet}… → …{snippet}…"
        .into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaypartEntry {
    pub word: String,
    pub rank: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaypartRegressionRule {
    #[serde(flatten)]
    pub meta: RuleMeta,
    #[serde(default = "default_regression_gap")]
    pub regression_gap: u8,
    #[serde(default = "default_dayparts")]
    pub dayparts: Vec<DaypartEntry>,
    #[serde(default = "default_overnight")]
    pub overnight_markers: Vec<String>,
    #[serde(default = "default_daypart_recall")]
    pub recall_markers: Vec<String>,
    #[serde(default = "default_schedule_patterns")]
    pub schedule_range_patterns: Vec<String>,
    #[serde(default = "default_daypart_msg")]
    pub message: String,
}

fn default_regression_gap() -> u8 {
    2
}
fn default_dayparts() -> Vec<DaypartEntry> {
    [
        ("凌晨", 0),
        ("黎明", 1),
        ("清晨", 2),
        ("早晨", 3),
        ("上午", 4),
        ("中午", 5),
        ("午后", 6),
        ("下午", 7),
        ("傍晚", 8),
        ("黄昏", 9),
        ("夜里", 10),
        ("夜晚", 10),
        ("深夜", 11),
        ("半夜", 12),
    ]
    .into_iter()
    .map(|(w, r)| DaypartEntry {
        word: w.into(),
        rank: r,
    })
    .collect()
}
fn default_overnight() -> Vec<String> {
    [
        "翌日", "次日", "第二天", "隔日", "天亮", "过了一夜", "一夜之后", "睡到",
        "醒来已是", "回忆", "想起", "梦里", "梦中", "那时", "当初",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
fn default_daypart_recall() -> Vec<String> {
    ["最初", "原先", "开始时", "曾经", "那时", "想起", "回忆", "梦里", "梦中"]
        .into_iter()
        .map(String::from)
        .collect()
}
fn default_schedule_patterns() -> Vec<String> {
    [
        "到凌晨", "至凌晨", "到黎明", "至黎明", "到清晨", "至清晨", "到早上", "至早上",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
fn default_daypart_msg() -> String {
    "章内时段叙述回跳：约第{prev_line}行已到「{prev_word}」，约第{line}行又写「{word}」，\
     且未见「翌日/天亮/过了一夜」等跨日交代。请理顺叙事时间或补跨日过渡。"
        .into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentRulesFile {
    #[serde(default)]
    pub rules: ContentRulesBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRulesBundle {
    #[serde(default = "default_banned_rule")]
    pub banned_name: BannedNameRule,
    #[serde(default = "default_contradiction_rule")]
    pub contradiction_marker: ContradictionRule,
    #[serde(default = "default_new_facts_rule")]
    pub too_many_new_facts: TooManyNewFactsRule,
    #[serde(default = "default_meta_rule")]
    pub meta_chapter_ref: MetaChapterRule,
    #[serde(default = "default_pipeline_meta_rule")]
    pub pipeline_meta_leak: PipelineMetaLeakRule,
    #[serde(default = "default_countdown_rule")]
    pub timeline_countdown_jump: CountdownJumpRule,
    #[serde(default = "default_daypart_rule")]
    pub timeline_daypart_regression: DaypartRegressionRule,
}

fn default_banned_rule() -> BannedNameRule {
    BannedNameRule {
        meta: RuleMeta {
            title: "禁名".into(),
            description: "正文不得出现 naming_rules.yaml 中的禁名。".into(),
            ..RuleMeta::default()
        },
    }
}
fn default_contradiction_rule() -> ContradictionRule {
    ContradictionRule {
        meta: RuleMeta {
            title: "矛盾标记残留".into(),
            description: "正文不得残留管道内部标记。".into(),
            ..RuleMeta::default()
        },
        marker: default_contradiction_marker(),
        message: default_contradiction_msg(),
    }
}
fn default_new_facts_rule() -> TooManyNewFactsRule {
    TooManyNewFactsRule {
        meta: RuleMeta {
            title: "新事实标记过多".into(),
            description: "单章 [NEW_FACT:…] 标记数量上限。".into(),
            ..RuleMeta::default()
        },
        max_count: default_max_new_facts(),
        pattern: default_new_fact_pattern(),
        message: default_new_facts_msg(),
    }
}
fn default_meta_rule() -> MetaChapterRule {
    MetaChapterRule {
        meta: RuleMeta {
            title: "章号元叙述".into(),
            description: "非标题行不得用「第N章」指称情节。".into(),
            ..RuleMeta::default()
        },
        pattern: default_meta_pattern(),
        allow_heading: true,
        rewrite_replacement: default_rewrite_replacement(),
        message: default_meta_msg(),
    }
}
fn default_pipeline_meta_rule() -> PipelineMetaLeakRule {
    PipelineMetaLeakRule {
        meta: RuleMeta {
            title: "管线元叙述泄露".into(),
            description: "正文不得出现章纲/大纲/设定文档对读等写作管线用语。".into(),
            ..RuleMeta::default()
        },
        patterns: default_pipeline_meta_patterns(),
        message: default_pipeline_meta_msg(),
    }
}
fn default_countdown_rule() -> CountdownJumpRule {
    CountdownJumpRule {
        meta: RuleMeta {
            title: "倒计时回跳".into(),
            description: "「还剩/剩余」当前读数须单调不增。".into(),
            ..RuleMeta::default()
        },
        recall_markers: default_countdown_recall(),
        message: default_countdown_msg(),
    }
}
fn default_daypart_rule() -> DaypartRegressionRule {
    DaypartRegressionRule {
        meta: RuleMeta {
            title: "时段叙述回跳".into(),
            description: "同日时段词不得明显回跳。".into(),
            ..RuleMeta::default()
        },
        regression_gap: default_regression_gap(),
        dayparts: default_dayparts(),
        overnight_markers: default_overnight(),
        recall_markers: default_daypart_recall(),
        schedule_range_patterns: default_schedule_patterns(),
        message: default_daypart_msg(),
    }
}

impl Default for ContentRulesBundle {
    fn default() -> Self {
        Self {
            banned_name: default_banned_rule(),
            contradiction_marker: default_contradiction_rule(),
            too_many_new_facts: default_new_facts_rule(),
            meta_chapter_ref: default_meta_rule(),
            pipeline_meta_leak: default_pipeline_meta_rule(),
            timeline_countdown_jump: default_countdown_rule(),
            timeline_daypart_regression: default_daypart_rule(),
        }
    }
}

/// Loaded content-rules config (engine + catalog for Web).
#[derive(Debug, Clone)]
pub struct ContentRulesConfig {
    pub rules: ContentRulesBundle,
    /// Raw YAML text as last loaded (for Web editor round-trip).
    pub raw_yaml: String,
}

impl Default for ContentRulesConfig {
    fn default() -> Self {
        let rules = ContentRulesBundle::default();
        let raw_yaml = serde_yaml::to_string(&ContentRulesFile {
            rules: rules.clone(),
        })
        .unwrap_or_default();
        Self { rules, raw_yaml }
    }
}

impl ContentRulesConfig {
    pub fn defaults() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            tracing::warn!(
                path = %path.display(),
                "content_rules.yaml missing; using embedded defaults"
            );
            return Self::defaults();
        };
        match serde_yaml::from_str::<ContentRulesFile>(&text) {
            Ok(file) => {
                tracing::info!("content rules loaded");
                Self {
                    rules: file.rules,
                    raw_yaml: text,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "content_rules.yaml parse failed; using defaults");
                Self::defaults()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        Self::load(&config_root.join("content_rules.yaml"))
    }

    /// Validate YAML text; returns parsed config or error message.
    pub fn parse_yaml(text: &str) -> Result<Self, String> {
        let file: ContentRulesFile =
            serde_yaml::from_str(text).map_err(|e| format!("content_rules.yaml 解析失败：{e}"))?;
        Ok(Self {
            rules: file.rules,
            raw_yaml: text.to_string(),
        })
    }

    /// Patch `enabled` / `blocking` for one rule id inside YAML text.
    /// Line-oriented so comments / ordering outside those keys are preserved.
    pub fn patch_rule_flags(
        yaml: &str,
        rule_id: &str,
        enabled: Option<bool>,
        blocking: Option<bool>,
    ) -> Result<String, String> {
        if enabled.is_none() && blocking.is_none() {
            return Err("未指定要修改的标志".into());
        }
        let known = [
            "banned_name",
            "contradiction_marker",
            "too_many_new_facts",
            "meta_chapter_ref",
            "pipeline_meta_leak",
            "timeline_countdown_jump",
            "timeline_daypart_regression",
        ];
        if !known.contains(&rule_id) {
            return Err(format!("未知规则 id：{rule_id}"));
        }
        let text = if yaml.trim().is_empty() {
            Self::defaults().raw_yaml
        } else {
            yaml.to_string()
        };
        let header = format!("  {rule_id}:");
        let lines: Vec<&str> = text.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.trim_end() == header || *l == header)
            .ok_or_else(|| format!("YAML 中未找到规则「{rule_id}」"))?;
        let mut end = lines.len();
        for (i, line) in lines.iter().enumerate().skip(start + 1) {
            // Next top-level key under `rules:` (two-space indent + name + ':')
            if line.starts_with("  ")
                && !line.starts_with("    ")
                && line.trim_end().ends_with(':')
                && !line.trim_start().starts_with('#')
            {
                end = i;
                break;
            }
            if !line.starts_with(' ') && !line.trim().is_empty() && !line.trim_start().starts_with('#')
            {
                end = i;
                break;
            }
        }

        let mut out: Vec<String> = lines.iter().take(start).map(|s| (*s).to_string()).collect();
        let mut block: Vec<String> = lines[start..end].iter().map(|s| (*s).to_string()).collect();
        let mut saw_enabled = false;
        let mut saw_blocking = false;
        for row in &mut block {
            let trim_len = row.len() - row.trim_start().len();
            let key = row.trim_start().to_string();
            if let Some(v) = enabled {
                if key.starts_with("enabled:") {
                    let indent = " ".repeat(trim_len);
                    *row = format!("{indent}enabled: {}", if v { "true" } else { "false" });
                    saw_enabled = true;
                    continue;
                }
            }
            if let Some(v) = blocking {
                if key.starts_with("blocking:") {
                    let indent = " ".repeat(trim_len);
                    *row = format!("{indent}blocking: {}", if v { "true" } else { "false" });
                    saw_blocking = true;
                }
            }
        }
        // Insert missing keys right after the rule header.
        if enabled.is_some() && !saw_enabled {
            block.insert(1, format!("    enabled: {}", if enabled.unwrap() { "true" } else { "false" }));
        }
        if blocking.is_some() && !saw_blocking {
            let insert_at = if block.len() > 1 { 2.min(block.len()) } else { 1 };
            block.insert(
                insert_at,
                format!(
                    "    blocking: {}",
                    if blocking.unwrap() { "true" } else { "false" }
                ),
            );
        }
        out.extend(block);
        out.extend(lines[end..].iter().map(|s| (*s).to_string()));
        let mut patched = out.join("\n");
        if text.ends_with('\n') && !patched.ends_with('\n') {
            patched.push('\n');
        }
        Self::parse_yaml(&patched)?;
        Ok(patched)
    }

    /// Catalog for Web UI (id / title / description / enabled / blocking).
    /// Display title for a rule id (Web / gate prompts). Unknown ids return the id itself.
    pub fn title_for_rule(&self, id: &str) -> String {
        let b = &self.rules;
        match id {
            "banned_name" => b.banned_name.meta.title.clone(),
            "contradiction_marker" => b.contradiction_marker.meta.title.clone(),
            "too_many_new_facts" => b.too_many_new_facts.meta.title.clone(),
            "meta_chapter_ref" => b.meta_chapter_ref.meta.title.clone(),
            "pipeline_meta_leak" => b.pipeline_meta_leak.meta.title.clone(),
            "timeline_countdown_jump" => b.timeline_countdown_jump.meta.title.clone(),
            "timeline_daypart_regression" => b.timeline_daypart_regression.meta.title.clone(),
            "body_state_side" => "身体侧别".into(),
            "body_state_locus" => "能力载体".into(),
            _ => id.to_string(),
        }
    }

    pub fn catalog(&self) -> Vec<serde_json::Value> {
        use serde_json::json;
        let b = &self.rules;
        vec![
            json!({
                "id": "banned_name",
                "title": b.banned_name.meta.title,
                "description": b.banned_name.meta.description,
                "enabled": b.banned_name.meta.enabled,
                "blocking": b.banned_name.meta.blocking,
            }),
            json!({
                "id": "contradiction_marker",
                "title": b.contradiction_marker.meta.title,
                "description": b.contradiction_marker.meta.description,
                "enabled": b.contradiction_marker.meta.enabled,
                "blocking": b.contradiction_marker.meta.blocking,
            }),
            json!({
                "id": "too_many_new_facts",
                "title": b.too_many_new_facts.meta.title,
                "description": b.too_many_new_facts.meta.description,
                "enabled": b.too_many_new_facts.meta.enabled,
                "blocking": b.too_many_new_facts.meta.blocking,
            }),
            json!({
                "id": "meta_chapter_ref",
                "title": b.meta_chapter_ref.meta.title,
                "description": b.meta_chapter_ref.meta.description,
                "enabled": b.meta_chapter_ref.meta.enabled,
                "blocking": b.meta_chapter_ref.meta.blocking,
            }),
            json!({
                "id": "pipeline_meta_leak",
                "title": b.pipeline_meta_leak.meta.title,
                "description": b.pipeline_meta_leak.meta.description,
                "enabled": b.pipeline_meta_leak.meta.enabled,
                "blocking": b.pipeline_meta_leak.meta.blocking,
            }),
            json!({
                "id": "timeline_countdown_jump",
                "title": b.timeline_countdown_jump.meta.title,
                "description": b.timeline_countdown_jump.meta.description,
                "enabled": b.timeline_countdown_jump.meta.enabled,
                "blocking": b.timeline_countdown_jump.meta.blocking,
            }),
            json!({
                "id": "timeline_daypart_regression",
                "title": b.timeline_daypart_regression.meta.title,
                "description": b.timeline_daypart_regression.meta.description,
                "enabled": b.timeline_daypart_regression.meta.enabled,
                "blocking": b.timeline_daypart_regression.meta.blocking,
            }),
        ]
    }
}

/// Convenience: embedded defaults (tests / missing config).
pub fn check_draft(draft: &str, banned_names: &[String]) -> Vec<ContentRuleViolation> {
    check_draft_with(&ContentRulesConfig::defaults(), draft, banned_names)
}

pub fn check_draft_with(
    cfg: &ContentRulesConfig,
    draft: &str,
    banned_names: &[String],
) -> Vec<ContentRuleViolation> {
    let mut out = Vec::new();
    let r = &cfg.rules;

    if r.banned_name.meta.enabled {
        for name in banned_names {
            if !name.is_empty() && draft.contains(name) {
                out.push(violation(
                    "banned_name",
                    format!("正文出现禁名「{name}」"),
                    r.banned_name.meta.blocking,
                ));
            }
        }
    }

    if r.contradiction_marker.meta.enabled {
        let marker = &r.contradiction_marker.marker;
        if !marker.is_empty() && draft.contains(marker) {
            let msg = r
                .contradiction_marker
                .message
                .replace("{marker}", marker);
            out.push(violation(
                "contradiction_marker",
                msg,
                r.contradiction_marker.meta.blocking,
            ));
        }
    }

    if r.too_many_new_facts.meta.enabled {
        if let Ok(re) = Regex::new(&r.too_many_new_facts.pattern) {
            let count = re.find_iter(draft).count();
            if count > r.too_many_new_facts.max_count {
                let msg = r
                    .too_many_new_facts
                    .message
                    .replace("{count}", &count.to_string());
                out.push(violation(
                    "too_many_new_facts",
                    msg,
                    r.too_many_new_facts.meta.blocking,
                ));
            }
        }
    }

    if r.meta_chapter_ref.meta.enabled {
        out.extend(check_meta_chapter_refs(cfg, draft));
    }
    if r.pipeline_meta_leak.meta.enabled {
        out.extend(check_pipeline_meta_leak(cfg, draft));
    }
    if r.timeline_countdown_jump.meta.enabled {
        out.extend(check_countdown_monotonicity(cfg, draft));
    }
    if r.timeline_daypart_regression.meta.enabled {
        out.extend(check_daypart_regression(cfg, draft));
    }
    out
}

fn check_pipeline_meta_leak(cfg: &ContentRulesConfig, draft: &str) -> Vec<ContentRuleViolation> {
    let rule = &cfg.rules.pipeline_meta_leak;
    let mut out = Vec::new();
    for (idx, line) in draft.lines().enumerate() {
        for pat in &rule.patterns {
            if pat.is_empty() {
                continue;
            }
            if line.contains(pat) {
                let msg = rule
                    .message
                    .replace("{match}", pat)
                    .replace("{line}", &(idx + 1).to_string());
                out.push(violation("pipeline_meta_leak", msg, rule.meta.blocking));
                break;
            }
        }
    }
    out
}

fn check_meta_chapter_refs(cfg: &ContentRulesConfig, draft: &str) -> Vec<ContentRuleViolation> {
    let rule = &cfg.rules.meta_chapter_ref;
    let Ok(re) = Regex::new(&rule.pattern) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, line) in draft.lines().enumerate() {
        let trimmed = line.trim_start();
        if rule.allow_heading && trimmed.starts_with('#') {
            continue;
        }
        if let Some(m) = re.find(line) {
            let msg = rule
                .message
                .replace("{match}", m.as_str())
                .replace("{line}", &(idx + 1).to_string());
            out.push(violation(
                "meta_chapter_ref",
                msg,
                rule.meta.blocking,
            ));
        }
    }
    out
}

/// Replace body「第N章」with configured replacement. Title headings kept.
pub fn rewrite_meta_chapter_refs_in_body(draft: &str) -> Option<String> {
    rewrite_meta_chapter_refs_with(&ContentRulesConfig::defaults(), draft)
}

pub fn rewrite_meta_chapter_refs_with(
    cfg: &ContentRulesConfig,
    draft: &str,
) -> Option<String> {
    let rule = &cfg.rules.meta_chapter_ref;
    if !rule.meta.enabled {
        return None;
    }
    let Ok(re) = Regex::new(&rule.pattern) else {
        return None;
    };
    let replacement = rule.rewrite_replacement.as_str();
    let mut changed = false;
    let mut out = String::with_capacity(draft.len());
    for (i, line) in draft.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if (rule.allow_heading && trimmed.starts_with('#')) || !re.is_match(line) {
            out.push_str(line);
            continue;
        }
        let replaced = re.replace_all(line, replacement);
        if replaced.as_ref() != line {
            changed = true;
        }
        out.push_str(&replaced);
    }
    if draft.ends_with('\n') {
        out.push('\n');
    }
    changed.then_some(out)
}

fn line_has_any(line: &str, markers: &[String]) -> bool {
    markers.iter().any(|m| !m.is_empty() && line.contains(m))
}

fn extract_countdown_readings(
    cfg: &ContentRulesConfig,
    draft: &str,
) -> Vec<(usize, u64, String)> {
    let recall = &cfg.rules.timeline_countdown_jump.recall_markers;
    let re_hms = Regex::new(
        r"(?:还剩|剩余)\s*约?\s*(\d+)\s*小时\s*(?:(\d+)\s*分)?\s*(?:(\d+)\s*秒)?",
    )
    .expect("countdown hms");
    let re_ms = Regex::new(r"(?:还剩|剩余)\s*约?\s*(\d+)\s*分\s*(\d+)\s*秒").expect("countdown ms");
    let re_m = Regex::new(r"(?:还剩|剩余)\s*约?\s*(\d+)\s*分").expect("countdown m");
    let mut out = Vec::new();
    for (idx, line) in draft.lines().enumerate() {
        if line.trim_start().starts_with('#') || line_has_any(line, recall) {
            continue;
        }
        let line_no = idx + 1;
        let mut matched_ms = false;
        for caps in re_hms.captures_iter(line) {
            let h: u64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
            let m: u64 = caps
                .get(2)
                .and_then(|x| x.as_str().parse().ok())
                .unwrap_or(0);
            let s: u64 = caps
                .get(3)
                .and_then(|x| x.as_str().parse().ok())
                .unwrap_or(0);
            let total = h * 3600 + m * 60 + s;
            let snippet: String = line.trim().chars().take(60).collect();
            out.push((line_no, total, snippet));
        }
        for caps in re_ms.captures_iter(line) {
            matched_ms = true;
            let m: u64 = caps.get(1).and_then(|x| x.as_str().parse().ok()).unwrap_or(0);
            let s: u64 = caps.get(2).and_then(|x| x.as_str().parse().ok()).unwrap_or(0);
            let total = m * 60 + s;
            let snippet: String = line.trim().chars().take(60).collect();
            out.push((line_no, total, snippet));
        }
        if matched_ms {
            continue;
        }
        for caps in re_m.captures_iter(line) {
            let m: u64 = caps.get(1).and_then(|x| x.as_str().parse().ok()).unwrap_or(0);
            let total = m * 60;
            let snippet: String = line.trim().chars().take(60).collect();
            out.push((line_no, total, snippet));
        }
    }
    out
}

fn check_countdown_monotonicity(
    cfg: &ContentRulesConfig,
    draft: &str,
) -> Vec<ContentRuleViolation> {
    let readings = extract_countdown_readings(cfg, draft);
    if readings.len() < 2 {
        return Vec::new();
    }
    let tmpl = &cfg.rules.timeline_countdown_jump.message;
    let mut out = Vec::new();
    let mut prev = &readings[0];
    for cur in readings.iter().skip(1) {
        if cur.1 > prev.1 {
            let msg = tmpl
                .replace("{prev_line}", &prev.0.to_string())
                .replace("{prev_secs}", &prev.1.to_string())
                .replace("{line}", &cur.0.to_string())
                .replace("{secs}", &cur.1.to_string())
                .replace("{prev_snippet}", &prev.2)
                .replace("{snippet}", &cur.2);
            out.push(violation(
                "timeline_countdown_jump",
                msg,
                cfg.rules.timeline_countdown_jump.meta.blocking,
            ));
        }
        prev = cur;
    }
    out
}

fn daypart_rank_cfg<'a>(cfg: &'a ContentRulesConfig, line: &str) -> Option<(&'a str, u8)> {
    // Prefer longer words first (stable sort by word len desc).
    let mut parts: Vec<&DaypartEntry> = cfg.rules.timeline_daypart_regression.dayparts.iter().collect();
    parts.sort_by(|a, b| b.word.chars().count().cmp(&a.word.chars().count()));
    for p in parts {
        if !p.word.is_empty() && line.contains(&p.word) {
            return Some((p.word.as_str(), p.rank));
        }
    }
    None
}

fn strip_quoted_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let closer = match c {
            '「' => Some('」'),
            '“' | '"' => Some('”'),
            '\'' => Some('\''),
            _ => None,
        };
        if let Some(end) = closer {
            for d in chars.by_ref() {
                if d == end || (end == '”' && d == '"') {
                    break;
                }
            }
            out.push(' ');
            continue;
        }
        out.push(c);
    }
    out
}

fn is_schedule_range(cfg: &ContentRulesConfig, line: &str) -> bool {
    cfg.rules
        .timeline_daypart_regression
        .schedule_range_patterns
        .iter()
        .any(|p| !p.is_empty() && line.contains(p))
}

fn check_daypart_regression(
    cfg: &ContentRulesConfig,
    draft: &str,
) -> Vec<ContentRuleViolation> {
    let rule = &cfg.rules.timeline_daypart_regression;
    let gap = rule.regression_gap;
    let mut out = Vec::new();
    let mut prev: Option<(usize, String, u8)> = None;
    for (idx, line) in draft.lines().enumerate() {
        if line.trim_start().starts_with('#') || line_has_any(line, &rule.recall_markers) {
            continue;
        }
        let narrative = strip_quoted_spans(line);
        if narrative.chars().all(|c| c.is_whitespace()) {
            continue;
        }
        if is_schedule_range(cfg, &narrative) || is_schedule_range(cfg, line) {
            continue;
        }
        let Some((word, rank)) = daypart_rank_cfg(cfg, &narrative) else {
            continue;
        };
        let line_no = idx + 1;
        if line_has_any(&narrative, &rule.overnight_markers)
            || line_has_any(line, &rule.overnight_markers)
        {
            prev = Some((line_no, word.to_string(), rank));
            continue;
        }
        if let Some((p_line, p_word, p_rank)) = &prev {
            if rank + gap < *p_rank {
                let msg = rule
                    .message
                    .replace("{prev_line}", &p_line.to_string())
                    .replace("{prev_word}", p_word)
                    .replace("{line}", &line_no.to_string())
                    .replace("{word}", word);
                out.push(violation(
                    "timeline_daypart_regression",
                    msg,
                    rule.meta.blocking,
                ));
            }
        }
        prev = Some((line_no, word.to_string(), rank));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_title_heading_but_flags_body_meta_chapter() {
        let draft = "# 第10章 开局\n\n第八章跟对方联系时，他感到不安。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "meta_chapter_ref"),
            "expected meta_chapter_ref, got {v:?}"
        );
    }

    #[test]
    fn flags_pipeline_meta_leak_outline_readback() {
        let draft = "# 第1章\n\n章纲里写的推进路径与他听到的并不一致。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "pipeline_meta_leak"),
            "expected pipeline_meta_leak, got {v:?}"
        );
    }

    #[test]
    fn flags_meta_chapter_with_spaces() {
        let draft = "# 第10章\n\n在第 9 章的那个瞬间，他停住了。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "meta_chapter_ref"),
            "expected spaced meta ref, got {v:?}"
        );
    }

    #[test]
    fn does_not_flag_numbered_proper_noun() {
        let draft = "# 第10章\n\n他想起第十三条军规与石碑上的记号。\n";
        let v = check_draft(draft, &[]);
        assert!(v.is_empty(), "got {v:?}");
    }

    #[test]
    fn rewrite_meta_chapter_keeps_heading() {
        let draft = "# 第10章 开局\n\n在第9章的那个瞬间，他停住了。\n";
        let fixed = rewrite_meta_chapter_refs_in_body(draft).expect("changed");
        assert!(fixed.contains("# 第10章 开局"));
        assert!(fixed.contains("在此前的那个瞬间"));
        assert!(
            check_draft(&fixed, &[]).is_empty(),
            "got {:?}",
            check_draft(&fixed, &[])
        );
    }

    #[test]
    fn flags_countdown_jump() {
        let draft = "\
# 第1章\n\n\
「还剩6分11秒。」他说。\n\n\
他走了几步。\n\n\
「还剩6分31秒。」屏幕又亮了。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "timeline_countdown_jump"),
            "expected countdown jump, got {v:?}"
        );
    }

    #[test]
    fn allows_monotonic_countdown() {
        let draft = "\
# 第1章\n\n\
「还剩6分31秒。」\n\n\
后来只剩「还剩6分11秒」。\n\n\
「还剩3分58秒。」\n";
        let v = check_draft(draft, &[]);
        assert!(
            !v.iter().any(|x| x.rule == "timeline_countdown_jump"),
            "got {v:?}"
        );
    }

    #[test]
    fn recall_countdown_not_treated_as_jump() {
        let draft = "\
# 第1章\n\n\
「还剩6分11秒。」\n\n\
他记得最初还剩6分31秒。\n";
        let v = check_draft(draft, &[]);
        assert!(
            !v.iter().any(|x| x.rule == "timeline_countdown_jump"),
            "got {v:?}"
        );
    }

    #[test]
    fn flags_daypart_regression() {
        let draft = "\
# 第1章\n\n\
深夜的风很冷。\n\n\
上午他又回到原地。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "timeline_daypart_regression"),
            "expected daypart regression, got {v:?}"
        );
    }

    #[test]
    fn allows_daypart_reset_with_overnight() {
        let draft = "\
# 第1章\n\n\
深夜的风很冷。\n\n\
翌日上午他又回到原地。\n";
        let v = check_draft(draft, &[]);
        assert!(
            !v.iter().any(|x| x.rule == "timeline_daypart_regression"),
            "got {v:?}"
        );
    }

    #[test]
    fn dialogue_schedule_凌晨_not_daypart_regression() {
        let draft = "\
# 第2章\n\n\
黄昏的光线斜着穿过银杏树的枝杈。\n\n\
“周主任，电表晚上十点到凌晨四点每天走字大约两度。”\n";
        let v = check_draft(draft, &[]);
        assert!(
            !v.iter().any(|x| x.rule == "timeline_daypart_regression"),
            "dialogue schedule must not count as TOD regression, got {v:?}"
        );
    }

    #[test]
    fn disabled_rule_skipped() {
        let mut cfg = ContentRulesConfig::defaults();
        cfg.rules.meta_chapter_ref.meta.enabled = false;
        let draft = "# 第10章\n\n第八章跟对方联系时，他感到不安。\n";
        let v = check_draft_with(&cfg, draft, &[]);
        assert!(
            !v.iter().any(|x| x.rule == "meta_chapter_ref"),
            "got {v:?}"
        );
    }

    #[test]
    fn non_blocking_violation_does_not_gate() {
        let mut cfg = ContentRulesConfig::defaults();
        cfg.rules.meta_chapter_ref.meta.blocking = false;
        let draft = "# 第10章\n\n第八章跟对方联系时，他感到不安。\n";
        let v = check_draft_with(&cfg, draft, &[]);
        assert!(v.iter().any(|x| x.rule == "meta_chapter_ref"));
        assert!(v.iter().all(|x| !x.blocking || x.rule != "meta_chapter_ref"));
        assert!(!has_blocking_violation(&v));
    }

    #[test]
    fn blocking_violation_gates() {
        let cfg = ContentRulesConfig::defaults();
        let draft = "# 第10章\n\n第八章跟对方联系时，他感到不安。\n";
        let v = check_draft_with(&cfg, draft, &[]);
        assert!(has_blocking_violation(&v));
        assert!(v.iter().any(|x| x.rule == "meta_chapter_ref" && x.blocking));
    }

    #[test]
    fn patch_rule_flags_toggles_enabled_and_blocking() {
        let yaml = ContentRulesConfig::defaults().raw_yaml;
        let patched =
            ContentRulesConfig::patch_rule_flags(&yaml, "meta_chapter_ref", Some(false), Some(false))
                .unwrap();
        let cfg = ContentRulesConfig::parse_yaml(&patched).unwrap();
        assert!(!cfg.rules.meta_chapter_ref.meta.enabled);
        assert!(!cfg.rules.meta_chapter_ref.meta.blocking);
        assert!(cfg.rules.banned_name.meta.enabled);
    }

    #[test]
    fn loads_repo_yaml_if_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let cfg = ContentRulesConfig::load_from_config_root(&root);
        assert!(cfg.rules.meta_chapter_ref.meta.enabled);
        assert!(!cfg.catalog().is_empty());
    }
}
