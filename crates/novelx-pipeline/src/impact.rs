//! Cross-artifact impact scan after a mutation is applied.
//! Deterministic keyword / name matching (optional LLM refine is feature-gated elsewhere).

use crate::cards::{entity_names_equivalent, load_markdown_cards};
use crate::project::{list_chapter_numbers, load_project_state, read_chapter_draft, read_chapter_outline};
use crate::volume::{active_volume_for_chapter, list_arc_outline_volumes, read_arc_outline_text};
use novelx_harness::ImpactScanMode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// When false (default), draft scans are limited to the active volume — unless
/// Entity/Bible mutations enable `studio.impact_scan_all_on_setting`.
fn feature_bool(project_dir: &Path, key: &str, default: bool) -> bool {
    let mut candidates = Vec::new();
    if let Some(root) = crate::volume_audit_gate::find_config_root(project_dir) {
        candidates.push(root.join("features.yaml"));
    }
    // Fallbacks for unusual layouts / cwd-based runs.
    candidates.push(project_dir.join("../../config/features.yaml"));
    candidates.push(project_dir.join("../config/features.yaml"));
    candidates.push(Path::new("config/features.yaml").to_path_buf());
    for path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            #[derive(Deserialize)]
            struct FeaturesFile {
                #[serde(default)]
                features: HashMap<String, bool>,
            }
            if let Ok(file) = serde_yaml::from_str::<FeaturesFile>(&raw) {
                return file.features.get(key).copied().unwrap_or(default);
            }
        }
    }
    default
}

fn impact_scan_all_drafts(project_dir: &Path) -> bool {
    feature_bool(project_dir, "studio.impact_scan_all_drafts", false)
}

fn impact_scan_all_on_setting(project_dir: &Path) -> bool {
    // Default false: prefer longform.yaml impact_scan_mode (indexed).
    feature_bool(project_dir, "studio.impact_scan_all_on_setting", false)
}

fn impact_scan_mode(project_dir: &Path) -> ImpactScanMode {
    if let Some(root) = crate::volume_audit_gate::find_config_root(project_dir) {
        return novelx_harness::LongformConfig::load_from_config_root(&root).impact_scan_mode;
    }
    for path in [
        project_dir.join("../../config/longform.yaml"),
        project_dir.join("../config/longform.yaml"),
        Path::new("config/longform.yaml").to_path_buf(),
    ] {
        if path.exists() {
            return novelx_harness::LongformConfig::load(&path).impact_scan_mode;
        }
    }
    ImpactScanMode::Indexed
}

#[derive(Debug, Clone, Default)]
pub struct ImpactScanOpts {
    /// Override volume vs all-drafts. None = feature/longform-driven.
    pub scan_all_drafts: Option<bool>,
    /// Override scan mode. None = longform.yaml / feature flags.
    pub scan_mode: Option<ImpactScanMode>,
}

fn scan_drafts_respecting_volume_flag(
    project_dir: &Path,
    keys: &[String],
    report: &mut ImpactReport,
    source_kind: &ImpactSourceKind,
    opts: &ImpactScanOpts,
) {
    // Explicit opt-in full scan via feature or opts.
    let force_all = opts.scan_all_drafts.unwrap_or_else(|| {
        if impact_scan_all_drafts(project_dir) {
            return true;
        }
        matches!(
            source_kind,
            ImpactSourceKind::Entity | ImpactSourceKind::Bible
        ) && impact_scan_all_on_setting(project_dir)
    });
    if force_all {
        scan_drafts(project_dir, keys, report);
        return;
    }

    let mode = opts.scan_mode.unwrap_or_else(|| impact_scan_mode(project_dir));
    match mode {
        ImpactScanMode::All => {
            scan_drafts(project_dir, keys, report);
        }
        ImpactScanMode::Volume => {
            let upto = load_project_state(project_dir)
                .map(|s| s.published_count.max(s.next_chapter.saturating_sub(1)).max(1))
                .unwrap_or_else(|_| list_chapter_numbers(project_dir).last().copied().unwrap_or(1));
            if let Some(vol) = active_volume_for_chapter(project_dir, upto) {
                scan_drafts_in_volume(project_dir, vol.volume_index, keys, report);
            } else {
                scan_drafts(project_dir, keys, report);
            }
        }
        ImpactScanMode::Indexed => {
            scan_drafts_indexed(project_dir, keys, report);
        }
    }
}

/// Active volume drafts + BM25/key-matched chapters elsewhere (longform default).
fn scan_drafts_indexed(project_dir: &Path, keys: &[String], report: &mut ImpactReport) {
    let upto = load_project_state(project_dir)
        .map(|s| s.published_count.max(s.next_chapter.saturating_sub(1)).max(1))
        .unwrap_or_else(|_| list_chapter_numbers(project_dir).last().copied().unwrap_or(1));
    let mut scanned: HashSet<u32> = HashSet::new();
    if let Some(vol) = active_volume_for_chapter(project_dir, upto) {
        let before = report.hits.len();
        scan_drafts_in_volume(project_dir, vol.volume_index, keys, report);
        for h in report.hits.iter().skip(before) {
            if let Some(ch) = h.chapter {
                scanned.insert(ch);
            }
        }
        // Also mark volume span as scanned even if no hits.
        let chapters = list_chapter_numbers(project_dir);
        let max_ch = chapters.iter().copied().max().unwrap_or(1);
        let bounds = crate::volume::load_volume_bounds(project_dir);
        if let Some(b) = crate::volume::bound_for_volume(&bounds, vol.volume_index) {
            let (from, to) = crate::volume::volume_chapter_span(&b, max_ch);
            for ch in from..=to {
                scanned.insert(ch);
            }
        }
    } else {
        // No volume → fall back to recent window only; indexed recall adds more.
        for ch in list_chapter_numbers(project_dir)
            .into_iter()
            .rev()
            .take(40)
        {
            scanned.insert(ch);
            scan_draft_chapter(project_dir, ch, keys, report);
        }
    }

    let query = keys.join(" ");
    let hits = crate::chapter_index::bm25_recall(project_dir, &query, keys, &[], 24);
    for dig in hits {
        if scanned.contains(&dig.chapter) {
            continue;
        }
        scanned.insert(dig.chapter);
        scan_draft_chapter(project_dir, dig.chapter, keys, report);
    }
}

/// What was mutated (source of the cascade).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImpactSourceKind {
    Entity,
    Bible,
    Outline,
    ArcOutline,
    MasterOutline,
    Draft,
}

impl ImpactSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Bible => "bible",
            Self::Outline => "outline",
            Self::ArcOutline => "arc_outline",
            Self::MasterOutline => "master_outline",
            Self::Draft => "draft",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "entity" => Some(Self::Entity),
            "bible" | "setting" | "worldview" => Some(Self::Bible),
            "outline" | "chapter_outline" | "revise_outline" => Some(Self::Outline),
            "arc_outline" | "design_arc_outline" => Some(Self::ArcOutline),
            "master_outline" | "design_master_outline" => Some(Self::MasterOutline),
            "draft" | "revise_chapter" => Some(Self::Draft),
            _ => None,
        }
    }
}

/// Dependency surface that may need revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ImpactTargetKind {
    Draft,
    Outline,
    ArcOutline,
    MasterOutline,
    Entity,
    Bible,
    Plot,
}

impl ImpactTargetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Outline => "outline",
            Self::ArcOutline => "arc_outline",
            Self::MasterOutline => "master_outline",
            Self::Entity => "entity",
            Self::Bible => "bible",
            Self::Plot => "plot",
        }
    }

    /// Cascade order: setting → outlines → drafts.
    pub fn cascade_rank(&self) -> u8 {
        match self {
            Self::Bible => 0,
            Self::Entity => 1,
            Self::MasterOutline => 2,
            Self::ArcOutline => 3,
            Self::Plot => 4,
            Self::Outline => 5,
            Self::Draft => 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactSource {
    pub kind: ImpactSourceKind,
    /// Display / lookup id (entity name, topic, chapter number as string, volume, …).
    pub id: String,
    #[serde(default)]
    pub entity_kind: String,
    #[serde(default)]
    pub chapter: Option<u32>,
    #[serde(default)]
    pub volume: Option<u32>,
    #[serde(default)]
    pub before_snippet: String,
    #[serde(default)]
    pub after_snippet: String,
    /// Names / aliases / rule phrases to search for.
    #[serde(default)]
    pub keys: Vec<String>,
}

impl ImpactSource {
    pub fn from_tool_data(data: &Value) -> Option<Self> {
        let raw = data.get("impact_source")?;
        let kind = raw
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(ImpactSourceKind::parse)?;
        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() && kind != ImpactSourceKind::Bible {
            return None;
        }
        let mut keys: Vec<String> = raw
            .get("keys")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let before = raw
            .get("before_snippet")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let after = raw
            .get("after_snippet")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        for p in stale_rule_phrases(&before, &after) {
            if !keys.iter().any(|k| k == &p) {
                keys.push(p);
            }
        }
        Some(Self {
            kind,
            id: if id.is_empty() { "bible".into() } else { id },
            entity_kind: raw
                .get("entity_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            chapter: raw
                .get("chapter")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
            volume: raw
                .get("volume")
                .or_else(|| raw.get("arc"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
            before_snippet: before,
            after_snippet: after,
            keys,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "id": self.id,
            "entity_kind": self.entity_kind,
            "chapter": self.chapter,
            "volume": self.volume,
            "before_snippet": self.before_snippet,
            "after_snippet": self.after_snippet,
            "keys": self.keys,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactHit {
    pub target_kind: ImpactTargetKind,
    /// Path or logical ref (e.g. chapters/001/draft.md, entities/items/X.md).
    pub target_ref: String,
    #[serde(default)]
    pub chapter: Option<u32>,
    #[serde(default)]
    pub volume: Option<u32>,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub reason: String,
    /// Key that matched.
    #[serde(default)]
    pub matched_key: String,
}

impl ImpactHit {
    pub fn to_json(&self) -> Value {
        json!({
            "target_kind": self.target_kind.as_str(),
            "target_ref": self.target_ref,
            "chapter": self.chapter,
            "volume": self.volume,
            "quote": self.quote,
            "reason": self.reason,
            "matched_key": self.matched_key,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImpactReport {
    pub hits: Vec<ImpactHit>,
    #[serde(default)]
    pub entity_gaps_count: usize,
}

impl ImpactReport {
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "hits": self.hits.iter().map(ImpactHit::to_json).collect::<Vec<_>>(),
            "entity_gaps_count": self.entity_gaps_count,
            "hit_count": self.hits.len(),
        })
    }

    pub fn summary_markdown(&self, max_hits: usize) -> String {
        if self.hits.is_empty() {
            return "未发现需同步的依赖位点。".into();
        }
        let mut lines = vec![format!(
            "发现 **{}** 处可能受影响的依赖位点：",
            self.hits.len()
        )];
        for (i, h) in self.hits.iter().take(max_hits).enumerate() {
            let where_ = match (h.chapter, h.volume) {
                (Some(c), _) => format!("第{c}章"),
                (_, Some(v)) => format!("第{v}卷"),
                _ => h.target_kind.as_str().to_string(),
            };
            let q: String = h.quote.chars().take(80).collect();
            lines.push(format!(
                "{}. [{} · {}] {} — 「{}」",
                i + 1,
                h.target_kind.as_str(),
                where_,
                h.reason,
                q
            ));
        }
        if self.hits.len() > max_hits {
            lines.push(format!("…另有 {} 处未列出", self.hits.len() - max_hits));
        }
        if self.entity_gaps_count > 0 {
            lines.push(format!(
                "\n设定缺口（启发式）当前约 {} 条；补洞请用 design_entity。",
                self.entity_gaps_count
            ));
        }
        lines.join("\n")
    }
}

/// Phrases present in before but not after (likely stale rules still in drafts).
pub fn stale_rule_phrases(before: &str, after: &str) -> Vec<String> {
    if before.trim().is_empty() {
        return Vec::new();
    }
    let after_n = normalize_ws(after);
    let mut out = BTreeSet::new();
    for line in before.lines() {
        let t = line.trim();
        if t.chars().count() < 4 || t.chars().count() > 40 {
            continue;
        }
        if t.starts_with('#') || t.starts_with("---") || t.starts_with("status:") {
            continue;
        }
        if after_n.contains(&normalize_ws(t)) {
            continue;
        }
        // Prefer constraint-looking lines.
        let constraint = ["次", "用", "毁", "限", "禁", "不可", "只能", "消耗", "冷却", "失效"]
            .iter()
            .any(|k| t.contains(k));
        if constraint || t.contains('：') || t.contains(':') {
            out.insert(t.to_string());
        }
    }
    out.into_iter().take(12).collect()
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("")
}

/// Scan project artifacts for impact of `source`.
pub fn scan_impact(project_dir: &Path, source: &ImpactSource) -> ImpactReport {
    scan_impact_with_opts(project_dir, source, &ImpactScanOpts::default())
}

/// Like [`scan_impact`], with controllable draft-scan scope for longform.
pub fn scan_impact_with_opts(
    project_dir: &Path,
    source: &ImpactSource,
    opts: &ImpactScanOpts,
) -> ImpactReport {
    let mut report = ImpactReport::default();
    report.entity_gaps_count = crate::cards::collect_entity_gaps(project_dir).len();

    // Draft changes: refresh gaps only (no reverse setting sweep).
    if source.kind == ImpactSourceKind::Draft {
        return report;
    }

    let keys = effective_keys(source);
    if keys.is_empty() && source.kind != ImpactSourceKind::MasterOutline {
        return report;
    }

    match source.kind {
        ImpactSourceKind::Entity | ImpactSourceKind::Bible => {
            scan_drafts_respecting_volume_flag(
                project_dir,
                &keys,
                &mut report,
                &source.kind,
                opts,
            );
            scan_outlines(project_dir, &keys, &mut report, None);
            scan_arc_outlines(project_dir, &keys, &mut report, None);
            if source.kind == ImpactSourceKind::Bible {
                scan_master_outline(project_dir, &keys, &mut report);
                scan_entities_for_keys(project_dir, &keys, &source.id, &mut report);
            }
            scan_plots(project_dir, &keys, &mut report);
        }
        ImpactSourceKind::Outline => {
            if let Some(ch) = source.chapter {
                scan_draft_chapter(project_dir, ch, &keys, &mut report);
                // Soft: active volume arc may drift.
                scan_arc_outlines(project_dir, &keys, &mut report, None);
                scan_plots(project_dir, &keys, &mut report);
            }
        }
        ImpactSourceKind::ArcOutline => {
            let vol = source.volume.unwrap_or(1);
            scan_outlines(project_dir, &keys, &mut report, Some(vol));
            scan_drafts_in_volume(project_dir, vol, &keys, &mut report);
            scan_plots(project_dir, &keys, &mut report);
        }
        ImpactSourceKind::MasterOutline => {
            // Only distinctive / stale phrases — never template H2 labels like「主线」「三幕」.
            let keys: Vec<String> = keys.into_iter().filter(|k| !is_template_section_key(k)).collect();
            if !keys.is_empty() {
                scan_arc_outlines(project_dir, &keys, &mut report, None);
                scan_plots(project_dir, &keys, &mut report);
            }
        }
        ImpactSourceKind::Draft => {}
    }

    report.hits.sort_by(|a, b| {
        a.target_kind
            .cascade_rank()
            .cmp(&b.target_kind.cascade_rank())
            .then(a.chapter.cmp(&b.chapter))
            .then(a.target_ref.cmp(&b.target_ref))
    });
    report.hits.dedup_by(|a, b| {
        a.target_kind == b.target_kind
            && a.target_ref == b.target_ref
            && a.matched_key == b.matched_key
    });
    // Cap to keep gate readable.
    if report.hits.len() > 40 {
        report.hits.truncate(40);
    }
    report
}

fn effective_keys(source: &ImpactSource) -> Vec<String> {
    let mut keys = source.keys.clone();
    if !source.id.is_empty()
        && source.id != "bible"
        && source.id != "master"
        && !source.id.starts_with("chapter_")
        && !source.id.starts_with("volume_")
        && !is_template_section_key(&source.id)
    {
        if !keys.iter().any(|k| entity_names_equivalent(k, &source.id)) {
            keys.insert(0, source.id.clone());
        }
    }
    keys.retain(|k| k.chars().count() >= 2 && !is_template_section_key(k));
    // Drop ultra-generic short tokens that flood false positives.
    keys.retain(|k| k.chars().count() >= 3 || entity_identity_parts_ok(k));
    keys
}

fn entity_identity_parts_ok(k: &str) -> bool {
    // Allow 2-char proper names (common CJK given names) only when not a template word.
    k.chars().count() == 2 && !is_template_section_key(k)
}

/// Outline/Bible section labels that appear in almost every artifact — never use as scan keys.
fn is_template_section_key(k: &str) -> bool {
    let t = k.trim().trim_start_matches('#').trim();
    matches!(
        t,
        "一句话卖点"
            | "三幕结构"
            | "分卷"
            | "主角弧"
            | "主线冲突"
            | "主线"
            | "三幕"
            | "总纲"
            | "世界观"
            | "卷目标"
            | "冲突阶梯"
            | "关键节点"
            | "人物弧"
            | "卷末终止条件"
            | "卷末交付"
            | "卷定位"
            | "力量体系"
            | "地理"
            | "总览"
            | "名词表"
            | "当前状态"
            | "能力"
            | "经历"
            | "性格"
    ) || t.starts_with("0.")
        || t.starts_with("1.")
        || t.starts_with("2.")
        || t.starts_with("7.")
}

fn extract_title_like_keys(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().take(40) {
        let t = line.trim().trim_start_matches('#').trim();
        if t.chars().count() < 4 || t.chars().count() > 24 || t.contains('。') {
            continue;
        }
        if is_template_section_key(t) {
            continue;
        }
        // Prefer content lines over bare section headers (no leading numbering-only).
        if t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ' ') {
            continue;
        }
        out.push(t.to_string());
        if out.len() >= 8 {
            break;
        }
    }
    out
}

fn text_mentions_key(text: &str, key: &str) -> bool {
    let key = key.trim();
    if key.is_empty() || text.is_empty() {
        return false;
    }
    if text.contains(key) {
        return true;
    }
    // Loose: identity parts for paren names.
    for part in crate::cards::entity_identity_parts(key) {
        if part.chars().count() >= 2 && text.contains(&part) {
            return true;
        }
    }
    false
}

fn quote_around(text: &str, key: &str, max: usize) -> String {
    if let Some(idx) = text.find(key) {
        let start = text[..idx]
            .char_indices()
            .rev()
            .nth(20)
            .map(|(i, _)| i)
            .unwrap_or(0);
        let end_byte = idx + key.len();
        let end = text[end_byte..]
            .char_indices()
            .nth(40)
            .map(|(i, _)| end_byte + i)
            .unwrap_or(text.len());
        let q: String = text[start..end].chars().take(max).collect();
        return q.replace('\n', " ");
    }
    text.chars().take(max).collect()
}

fn push_hit(
    report: &mut ImpactReport,
    target_kind: ImpactTargetKind,
    target_ref: String,
    chapter: Option<u32>,
    volume: Option<u32>,
    text: &str,
    key: &str,
    reason: &str,
) {
    if report
        .hits
        .iter()
        .any(|h| h.target_ref == target_ref && h.matched_key == key)
    {
        return;
    }
    report.hits.push(ImpactHit {
        target_kind,
        target_ref,
        chapter,
        volume,
        quote: quote_around(text, key, 100),
        reason: reason.to_string(),
        matched_key: key.to_string(),
    });
}

fn scan_drafts(project_dir: &Path, keys: &[String], report: &mut ImpactReport) {
    for ch in list_chapter_numbers(project_dir) {
        scan_draft_chapter(project_dir, ch, keys, report);
    }
}

fn scan_draft_chapter(project_dir: &Path, ch: u32, keys: &[String], report: &mut ImpactReport) {
    let Some(draft) = read_chapter_draft(project_dir, ch) else {
        return;
    };
    if draft.trim().is_empty() {
        return;
    }
    for key in keys {
        if text_mentions_key(&draft, key) {
            // Prefer paragraph-level quote.
            let para = draft
                .split("\n\n")
                .find(|p| text_mentions_key(p, key))
                .unwrap_or(&draft);
            push_hit(
                report,
                ImpactTargetKind::Draft,
                format!("chapters/{ch:03}/draft.md"),
                Some(ch),
                None,
                para,
                key,
                "正文提及相关设定/旧规则",
            );
        }
    }
}

fn scan_drafts_in_volume(
    project_dir: &Path,
    volume: u32,
    keys: &[String],
    report: &mut ImpactReport,
) {
    let chapters = list_chapter_numbers(project_dir);
    let upto = chapters.iter().copied().max().unwrap_or(1);
    let bounds = crate::volume::load_volume_bounds(project_dir);
    let (from, to) = crate::volume::bound_for_volume(&bounds, volume)
        .map(|b| crate::volume::volume_chapter_span(&b, upto))
        .unwrap_or((1, upto));
    for ch in chapters {
        if ch < from || ch > to {
            continue;
        }
        let before = report.hits.len();
        scan_draft_chapter(project_dir, ch, keys, report);
        for h in report.hits.iter_mut().skip(before) {
            if h.chapter == Some(ch) {
                h.volume = Some(volume);
            }
        }
    }
}

fn scan_outlines(
    project_dir: &Path,
    keys: &[String],
    report: &mut ImpactReport,
    volume_filter: Option<u32>,
) {
    let chapters = list_chapter_numbers(project_dir);
    let upto = chapters.iter().copied().max().unwrap_or(1);
    let bounds = crate::volume::load_volume_bounds(project_dir);
    let span = volume_filter.and_then(|vol| {
        crate::volume::bound_for_volume(&bounds, vol)
            .map(|b| crate::volume::volume_chapter_span(&b, upto))
    });
    for ch in chapters {
        if let Some((from, to)) = span {
            if ch < from || ch > to {
                continue;
            }
        }
        let Some(outline) = read_chapter_outline(project_dir, ch) else {
            continue;
        };
        if outline.trim().is_empty() {
            continue;
        }
        for key in keys {
            if text_mentions_key(&outline, key) {
                push_hit(
                    report,
                    ImpactTargetKind::Outline,
                    format!("chapters/{ch:03}/outline.json"),
                    Some(ch),
                    volume_filter,
                    &outline,
                    key,
                    "章纲提及相关设定/旧规则",
                );
            }
        }
    }
}

fn scan_arc_outlines(
    project_dir: &Path,
    keys: &[String],
    report: &mut ImpactReport,
    only_volume: Option<u32>,
) {
    let volumes: Vec<u32> = if let Some(v) = only_volume {
        vec![v]
    } else {
        list_arc_outline_volumes(project_dir)
    };
    for vol in volumes {
        let Some(text) = read_arc_outline_text(project_dir, vol) else {
            continue;
        };
        for key in keys {
            if text_mentions_key(&text, key) {
                push_hit(
                    report,
                    ImpactTargetKind::ArcOutline,
                    format!("artifacts/arc_outlines/{vol:02}.md"),
                    None,
                    Some(vol),
                    &text,
                    key,
                    "卷纲提及相关设定/旧规则",
                );
            }
        }
    }
}

fn scan_master_outline(project_dir: &Path, keys: &[String], report: &mut ImpactReport) {
    let path = project_dir.join("artifacts/master_outline.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for key in keys {
        if text_mentions_key(&text, key) {
            push_hit(
                report,
                ImpactTargetKind::MasterOutline,
                "artifacts/master_outline.md".into(),
                None,
                None,
                &text,
                key,
                "总纲提及相关设定/旧规则",
            );
        }
    }
}

fn scan_entities_for_keys(
    project_dir: &Path,
    keys: &[String],
    skip_id: &str,
    report: &mut ImpactReport,
) {
    for group in ["characters", "items", "locations"] {
        let folder = project_dir.join("entities").join(group);
        for card in load_markdown_cards(&folder, group) {
            if entity_names_equivalent(&card.name, skip_id) {
                continue;
            }
            for key in keys {
                if key.chars().count() < 3 {
                    continue;
                }
                if text_mentions_key(&card.markdown, key) {
                    push_hit(
                        report,
                        ImpactTargetKind::Entity,
                        format!("entities/{group}/{}.md", card.slug),
                        None,
                        None,
                        &card.markdown,
                        key,
                        "实体卡可能与世界观变更冲突",
                    );
                }
            }
        }
    }
}

fn scan_plots(project_dir: &Path, keys: &[String], report: &mut ImpactReport) {
    let folder = project_dir.join("plots");
    let Ok(rd) = std::fs::read_dir(&folder) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for key in keys {
            if text_mentions_key(&text, key) {
                push_hit(
                    report,
                    ImpactTargetKind::Plot,
                    format!("plots/{}", path.file_name().unwrap_or_default().to_string_lossy()),
                    None,
                    None,
                    &text,
                    key,
                    "剧情卡提及相关设定/旧规则",
                );
            }
        }
    }
}

/// Build impact_source JSON helpers for tools.
pub fn impact_source_entity(
    kind: &str,
    name: &str,
    keys: Vec<String>,
    before: &str,
    after: &str,
) -> Value {
    let mut keys = keys;
    if !keys.iter().any(|k| entity_names_equivalent(k, name)) {
        keys.insert(0, name.to_string());
    }
    for p in stale_rule_phrases(before, after) {
        if !keys.iter().any(|k| k == &p) {
            keys.push(p);
        }
    }
    json!({
        "kind": "entity",
        "id": name,
        "entity_kind": kind,
        "before_snippet": before.chars().take(2000).collect::<String>(),
        "after_snippet": after.chars().take(2000).collect::<String>(),
        "keys": keys,
    })
}

pub fn impact_source_bible(topic: &str, before: &str, after: &str) -> Value {
    let mut keys = Vec::new();
    // Topic is often a section label (力量体系) — only keep if distinctive & non-template.
    if !is_template_section_key(topic) && topic.chars().count() >= 2 {
        keys.push(topic.to_string());
    }
    for p in stale_rule_phrases(before, after) {
        if !is_template_section_key(&p) {
            keys.push(p);
        }
    }
    // Distinctive body lines from the *diff*, not H2 templates from after.
    for line in after.lines().take(40) {
        let t = line.trim().trim_start_matches('#').trim();
        if t.chars().count() < 6 || t.chars().count() > 28 {
            continue;
        }
        if is_template_section_key(t) {
            continue;
        }
        // Skip lines that also appear unchanged in before (noise).
        if normalize_ws(before).contains(&normalize_ws(t)) {
            continue;
        }
        keys.push(t.to_string());
        if keys.len() >= 12 {
            break;
        }
    }
    json!({
        "kind": "bible",
        "id": topic,
        "before_snippet": before.chars().take(2000).collect::<String>(),
        "after_snippet": after.chars().take(2000).collect::<String>(),
        "keys": keys,
    })
}

pub fn impact_source_outline(chapter: u32, before: &str, after: &str) -> Value {
    let mut keys = Vec::new();
    for p in stale_rule_phrases(before, after) {
        keys.push(p);
    }
    // Character/item name-ish tokens from JSON strings.
    for chunk in after.split(|c: char| c == '"' || c == ',' || c == '[' || c == ']') {
        let t = chunk.trim();
        if t.chars().count() >= 2 && t.chars().count() <= 12 && !t.contains(':') {
            if t.chars().all(|c| !c.is_ascii_alphanumeric() || c.is_ascii_alphabetic()) {
                // keep CJK-heavy short tokens
                let cjk = t.chars().filter(|c| *c > '\u{4e00}').count();
                if cjk >= 2 {
                    keys.push(t.to_string());
                }
            }
        }
        if keys.len() >= 20 {
            break;
        }
    }
    json!({
        "kind": "outline",
        "id": format!("chapter_{chapter}"),
        "chapter": chapter,
        "before_snippet": before.chars().take(1500).collect::<String>(),
        "after_snippet": after.chars().take(1500).collect::<String>(),
        "keys": keys,
    })
}

pub fn impact_source_arc(volume: u32, before: &str, after: &str) -> Value {
    let mut keys = Vec::new();
    for p in stale_rule_phrases(before, after) {
        if !is_template_section_key(&p) {
            keys.push(p);
        }
    }
    for k in extract_title_like_keys(after) {
        if normalize_ws(before).contains(&normalize_ws(&k)) {
            continue;
        }
        keys.push(k);
    }
    json!({
        "kind": "arc_outline",
        "id": format!("volume_{volume}"),
        "volume": volume,
        "before_snippet": before.chars().take(2000).collect::<String>(),
        "after_snippet": after.chars().take(2000).collect::<String>(),
        "keys": keys,
    })
}

pub fn impact_source_master(before: &str, after: &str) -> Value {
    let mut keys = Vec::new();
    for p in stale_rule_phrases(before, after) {
        if !is_template_section_key(&p) {
            keys.push(p);
        }
    }
    for k in extract_title_like_keys(after) {
        if normalize_ws(before).contains(&normalize_ws(&k)) {
            continue;
        }
        keys.push(k);
    }
    json!({
        "kind": "master_outline",
        "id": "master",
        "before_snippet": before.chars().take(2000).collect::<String>(),
        "after_snippet": after.chars().take(2000).collect::<String>(),
        "keys": keys,
    })
}

pub fn impact_source_draft(chapter: u32) -> Value {
    json!({
        "kind": "draft",
        "id": format!("chapter_{chapter}"),
        "chapter": chapter,
        "keys": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("novelx-impact-{tag}-{stamp}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn stale_phrases_detect_removed_rule() {
        let before = "## 能力\n玉佩一次性使用，用完即毁。\n";
        let after = "## 能力\n玉佩可用七次，每次消耗一缕灵气。\n";
        let phrases = stale_rule_phrases(before, after);
        assert!(
            phrases.iter().any(|p| p.contains("用完即毁") || p.contains("一次性")),
            "{phrases:?}"
        );
    }

    #[test]
    fn indexed_mode_scans_active_and_recalled() {
        let root = tmp_root("indexed");
        fs::create_dir_all(root.join("chapters/001")).unwrap();
        fs::create_dir_all(root.join("chapters/050")).unwrap();
        fs::create_dir_all(root.join("lore")).unwrap();
        fs::create_dir_all(root.join("artifacts/arc_outlines")).unwrap();
        fs::write(
            root.join("state.json"),
            r#"{"name":"sample-novel","genre":"未定","target_chapters":900,"published_count":50,"next_chapter":51,"active_agents":[]}"#,
        )
        .unwrap();
        fs::write(
            root.join("artifacts/arc_outlines/01.md"),
            "# 卷一\n\n## 终止条件\n- 抵达落点\n- 目击异象\n",
        )
        .unwrap();
        // Minimal story_outline so volume bounds exist for ch 50.
        fs::write(
            root.join("artifacts/story_outline.json"),
            r#"{"acts":[{"volume_index":1,"start_chapter":1,"end_chapter":60,"name":"卷一"}]}"#,
        )
        .unwrap();
        fs::write(
            root.join("chapters/001/draft.md"),
            "主角带着旧令牌离开。\n",
        )
        .unwrap();
        fs::write(
            root.join("chapters/050/draft.md"),
            "他想起旧令牌还在怀里。\n",
        )
        .unwrap();
        fs::write(
            root.join("lore/chapter_index.json"),
            r#"{"version":1,"chapters":[{"chapter":1,"title":"","tokens":["旧令牌","主角"],"event_summary":"旧令牌出场","hook":"","fingerprint":""}]}"#,
        )
        .unwrap();
        let source = ImpactSource {
            kind: ImpactSourceKind::Entity,
            id: "旧令牌".into(),
            entity_kind: "item".into(),
            chapter: None,
            volume: None,
            before_snippet: String::new(),
            after_snippet: String::new(),
            keys: vec!["旧令牌".into()],
        };
        let report = scan_impact_with_opts(
            &root,
            &source,
            &ImpactScanOpts {
                scan_all_drafts: Some(false),
                scan_mode: Some(ImpactScanMode::Indexed),
            },
        );
        assert!(
            report.hits.iter().any(|h| h.chapter == Some(50)),
            "active volume draft should hit: {:?}",
            report.hits
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_entity_hits_draft_and_outline() {
        let root = tmp_root("entity");
        fs::create_dir_all(root.join("chapters/001")).unwrap();
        fs::create_dir_all(root.join("entities/items")).unwrap();
        fs::write(
            root.join("chapters/001/draft.md"),
            "主角摸出样例玉佩。样例玉佩用完即毁，光芒散尽。\n",
        )
        .unwrap();
        fs::write(
            root.join("chapters/001/outline.json"),
            r#"{"title":"试炼","characters":["主角"],"items":["样例玉佩"],"key_events":["使用样例玉佩","用完即毁"]}"#,
        )
        .unwrap();
        fs::write(
            root.join("entities/items/样例玉佩.md"),
            "---\nname: 样例玉佩\n---\n# 样例玉佩\n可用七次。\n",
        )
        .unwrap();

        let source = ImpactSource {
            kind: ImpactSourceKind::Entity,
            id: "样例玉佩".into(),
            entity_kind: "item".into(),
            chapter: None,
            volume: None,
            before_snippet: "一次性使用，用完即毁。".into(),
            after_snippet: "可用七次。".into(),
            keys: vec!["样例玉佩".into(), "用完即毁".into()],
        };
        let report = scan_impact(&root, &source);
        assert!(
            report.hits.iter().any(|h| h.target_kind == ImpactTargetKind::Draft),
            "{:?}",
            report.hits
        );
        assert!(
            report
                .hits
                .iter()
                .any(|h| h.target_kind == ImpactTargetKind::Outline),
            "{:?}",
            report.hits
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn master_template_keys_do_not_flood_hits() {
        let root = tmp_root("master-keys");
        fs::create_dir_all(root.join("artifacts/arc_outlines")).unwrap();
        fs::write(
            root.join("artifacts/arc_outlines/01.md"),
            "# 第1卷 · 开局\n## 卷目标\n抵达落点。\n## 主线冲突\n目击异象。\n## 卷末终止条件\n- a\n- b\n",
        )
        .unwrap();
        let source = ImpactSource {
            kind: ImpactSourceKind::MasterOutline,
            id: "master".into(),
            entity_kind: String::new(),
            chapter: None,
            volume: None,
            before_snippet: "# 总纲\n## 一句话卖点\n旧卖点\n## 主线冲突\n旧冲突\n".into(),
            after_snippet: "# 总纲\n## 一句话卖点\n新卖点\n## 主线冲突\n新冲突\n".into(),
            keys: vec!["一句话卖点".into(), "主线".into(), "三幕".into()],
        };
        let report = scan_impact(&root, &source);
        assert!(
            report.hits.is_empty(),
            "template keys must not hit arcs: {:?}",
            report.hits
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn draft_source_only_counts_gaps() {
        let root = tmp_root("draft");
        fs::create_dir_all(root.join("chapters/001")).unwrap();
        fs::write(root.join("chapters/001/draft.md"), "主角到了甲地。").unwrap();
        let source = ImpactSource {
            kind: ImpactSourceKind::Draft,
            id: "chapter_1".into(),
            entity_kind: String::new(),
            chapter: Some(1),
            volume: None,
            before_snippet: String::new(),
            after_snippet: String::new(),
            keys: vec!["主角".into()],
        };
        let report = scan_impact(&root, &source);
        assert!(report.hits.is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
