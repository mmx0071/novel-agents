//! Local-first draft patching — paragraph grep + span replacement.
//! Prefer local spans over full-chapter rewrites.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionTarget {
    pub instruction: String,
    /// 0-based inclusive paragraph start
    pub start: usize,
    /// 0-based inclusive paragraph end
    pub end: usize,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub source: String,
}

impl RevisionTarget {
    pub fn para_label(&self) -> String {
        if self.source == "timeline_batch" {
            let n = self.batch_para_indices().len().max(1);
            return format!("时间锚点×{n}");
        }
        let a = self.start + 1;
        let b = self.end + 1;
        if a == b {
            format!("第{a}段")
        } else {
            format!("第{a}–{b}段")
        }
    }

    /// Discrete 0-based paragraph indices for `timeline_batch` (from `quote`).
    pub fn batch_para_indices(&self) -> Vec<usize> {
        if self.source != "timeline_batch" {
            return (self.start..=self.end).collect();
        }
        let mut idxs: Vec<usize> = self
            .quote
            .split(|c: char| c == ',' || c == '，' || c.is_whitespace())
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if idxs.is_empty() {
            return (self.start..=self.end).collect();
        }
        idxs.sort_unstable();
        idxs.dedup();
        idxs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchResult {
    pub draft: String,
    pub applied: Vec<AppliedPatch>,
    pub failed: Vec<String>,
    /// true when at least one span was applied
    pub used_local_patch: bool,
    /// true when caller should fall back to full rewrite
    pub needs_full_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedPatch {
    pub start: usize,
    pub end: usize,
    pub before: String,
    pub after: String,
    pub instruction: String,
}

pub fn segment_paragraphs(draft: &str) -> Vec<String> {
    let re = Regex::new(r"\n\s*\n+").unwrap();
    let chunks: Vec<String> = re
        .split(draft)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if !chunks.is_empty() {
        return chunks;
    }
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let lines: Vec<&str> = trimmed.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut out = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    for ln in lines {
        buf.push(ln);
        let joined: String = buf.concat();
        if joined.chars().count() >= 180 {
            out.push(if buf.len() == 1 {
                buf[0].to_string()
            } else {
                buf.join("\n")
            });
            buf.clear();
        }
    }
    if !buf.is_empty() {
        out.push(buf.join("\n"));
    }
    out
}

fn norm(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn grep_paragraph_index(
    paragraphs: &[String],
    para_num: Option<usize>,
    quote: &str,
) -> Option<usize> {
    let n = paragraphs.len();
    if n == 0 {
        return None;
    }
    if let Some(pn) = para_num {
        if (1..=n).contains(&pn) {
            let idx = pn - 1;
            let q = quote.trim();
            if q.is_empty() {
                return Some(idx);
            }
            if paragraphs[idx].contains(q) || norm(&paragraphs[idx]).contains(&norm(q)) {
                return Some(idx);
            }
        }
    }
    let q = quote.trim();
    if q.is_empty() {
        return para_num.filter(|pn| (1..=n).contains(pn)).map(|pn| pn - 1);
    }
    for (i, p) in paragraphs.iter().enumerate() {
        if p.contains(q) {
            return Some(i);
        }
    }
    let nq = norm(q);
    if nq.is_empty() {
        return None;
    }
    for (i, p) in paragraphs.iter().enumerate() {
        if norm(p).contains(&nq) {
            return Some(i);
        }
    }
    for length in [28usize, 20, 16, 12] {
        if nq.chars().count() < length {
            continue;
        }
        let anchor: String = nq.chars().take(length).collect();
        let hits: Vec<usize> = paragraphs
            .iter()
            .enumerate()
            .filter(|(_, p)| norm(p).contains(&anchor))
            .map(|(i, _)| i)
            .collect();
        if hits.len() == 1 {
            return Some(hits[0]);
        }
    }
    None
}

pub fn parse_para_range(location: &str) -> Option<(usize, usize)> {
    let text = location.trim();
    if text.is_empty() {
        return None;
    }
    // 第2段至第4段 / 第2段到第4段 / 第2段-第4段 / 第2段—4段
    static RE_RANGE: OnceLock<Regex> = OnceLock::new();
    let re_range = RE_RANGE.get_or_init(|| {
        Regex::new(r"第\s*(\d+)\s*段\s*(?:至|到|[-–—~])\s*第?\s*(\d+)\s*段?").unwrap()
    });
    if let Some(c) = re_range.captures(text) {
        let a: usize = c[1].parse().ok()?;
        let b: usize = c[2].parse().ok()?;
        return Some((a.min(b), a.max(b)));
    }
    // Compact: 第1—2段 / 第1-2段（审校模型常用，中间无「段」）
    static RE_COMPACT: OnceLock<Regex> = OnceLock::new();
    let re_compact = RE_COMPACT.get_or_init(|| {
        Regex::new(r"第\s*(\d+)\s*[-–—~]\s*(\d+)\s*段").unwrap()
    });
    if let Some(c) = re_compact.captures(text) {
        let a: usize = c[1].parse().ok()?;
        let b: usize = c[2].parse().ok()?;
        return Some((a.min(b), a.max(b)));
    }
    static RE_SINGLE: OnceLock<Regex> = OnceLock::new();
    let re_single = RE_SINGLE.get_or_init(|| Regex::new(r"第\s*(\d+)\s*段").unwrap());
    if let Some(c) = re_single.captures(text) {
        let a: usize = c[1].parse().ok()?;
        return Some((a, a));
    }
    None
}

pub fn extract_quotes(text: &str) -> Vec<String> {
    let patterns = [
        Regex::new(r"「([^」]{2,80})」").unwrap(),
        Regex::new(r"『([^』]{2,80})』").unwrap(),
        Regex::new(r#""([^"]{2,80})""#).unwrap(),
    ];
    let mut out = Vec::new();
    for re in &patterns {
        for c in re.captures_iter(text) {
            if let Some(m) = c.get(1) {
                out.push(m.as_str().to_string());
            }
        }
    }
    out
}

pub fn resolve_span(
    paragraphs: &[String],
    location: &str,
    quote: &str,
) -> Option<(usize, usize)> {
    if let Some((a, b)) = parse_para_range(location) {
        let start = a.saturating_sub(1);
        let end = b.saturating_sub(1).min(paragraphs.len().saturating_sub(1));
        if start < paragraphs.len() {
            return Some((start, end));
        }
    }
    if let Some(idx) = grep_paragraph_index(paragraphs, None, quote) {
        return Some((idx, idx));
    }
    // Auditor often joins two spans with …… / ... — try each fragment.
    for frag in quote
        .split("……")
        .flat_map(|s| s.split('…'))
        .flat_map(|s| s.split("..."))
        .map(str::trim)
        .filter(|s| s.chars().count() >= 8)
    {
        if let Some(idx) = grep_paragraph_index(paragraphs, None, frag) {
            return Some((idx, idx));
        }
    }
    for q in extract_quotes(location) {
        if let Some(idx) = grep_paragraph_index(paragraphs, None, &q) {
            return Some((idx, idx));
        }
    }
    None
}

/// Render audit issues into a revision brief (for full-rewrite fallback).
pub fn format_audit_issues_brief(issues: &[serde_json::Value]) -> String {
    if issues.is_empty() {
        return String::new();
    }
    let mut lines = vec!["必须逐条修复下列审校问题（不可忽略）：".to_string()];
    for (i, issue) in issues.iter().enumerate() {
        let pri = issue
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("P1");
        let msg = issue
            .get("message")
            .or_else(|| issue.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("修复一致性问题");
        let loc = issue
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let quote = issue.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        let mut line = format!("{}. [{}] {}", i + 1, pri, msg);
        if !loc.is_empty() {
            line.push_str(&format!("（{loc}）"));
        }
        if !quote.is_empty() {
            let q: String = quote.chars().take(80).collect();
            line.push_str(&format!(" 原文：「{q}」"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Parse free-form user instructions into revision targets (local-first).
pub fn targets_from_user_instructions(draft: &str, instructions: &str) -> Vec<RevisionTarget> {
    let paragraphs = segment_paragraphs(draft);
    if paragraphs.is_empty() || instructions.trim().is_empty() {
        return vec![];
    }
    let mut targets = Vec::new();
    if let Some((start, end)) = resolve_span(&paragraphs, instructions, "") {
        targets.push(RevisionTarget {
            instruction: instructions.trim().to_string(),
            start,
            end,
            quote: String::new(),
            source: "user_instructions".into(),
        });
        return targets;
    }
    for q in extract_quotes(instructions) {
        if let Some(idx) = grep_paragraph_index(&paragraphs, None, &q) {
            targets.push(RevisionTarget {
                instruction: instructions.trim().to_string(),
                start: idx,
                end: idx,
                quote: q,
                source: "user_quote".into(),
            });
            break;
        }
    }
    targets
}

/// Build a local-revise instruction from pacing JSON (detail/action/quote/suggestion).
pub fn pacing_suggestion_instruction(sug: &serde_json::Value) -> String {
    let detail = sug
        .get("detail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let suggestion = sug
        .get("suggestion")
        .or_else(|| sug.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let action = sug
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let quote = sug
        .get("quote")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let body = if !detail.is_empty() {
        detail
    } else if !suggestion.is_empty() {
        suggestion
    } else {
        "调整节奏"
    };
    let mut parts = Vec::new();
    if !action.is_empty() && !body.contains(action) {
        parts.push(format!("动作：{action}"));
    }
    if !quote.is_empty() && !body.contains(quote) {
        parts.push(format!("锚点：「{quote}」"));
    }
    parts.push(body.to_string());
    parts.join("；")
}

pub fn collect_revision_targets(
    draft: &str,
    user_instructions: Option<&str>,
    audit_issues: &[serde_json::Value],
    pacing_suggestions: &[serde_json::Value],
) -> Vec<RevisionTarget> {
    let paragraphs = segment_paragraphs(draft);
    let mut targets = Vec::new();

    if let Some(instr) = user_instructions {
        targets.extend(targets_from_user_instructions(draft, instr));
    }

    for issue in audit_issues {
        let instruction = issue
            .get("message")
            .or_else(|| issue.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("修复一致性问题")
            .to_string();
        let location = issue
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let quote = issue.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        if let Some((start, end)) = resolve_span(&paragraphs, location, quote) {
            targets.push(RevisionTarget {
                instruction,
                start,
                end,
                quote: quote.to_string(),
                source: "audit".into(),
            });
        }
    }

    for sug in pacing_suggestions {
        let instruction = pacing_suggestion_instruction(sug);
        let quote = sug.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        let location = sug
            .get("location")
            .or_else(|| sug.get("paragraph"))
            .map(|v| match v {
                serde_json::Value::Number(n) => format!("第{}段", n),
                serde_json::Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        if let Some((start, end)) = resolve_span(&paragraphs, &location, quote) {
            targets.push(RevisionTarget {
                instruction,
                start,
                end,
                quote: quote.to_string(),
                source: "pacing".into(),
            });
        }
    }

    expand_timeline_issue_targets(draft, audit_issues, &mut targets);
    coalesce_timeline_batch(merge_overlapping(targets))
}

fn looks_like_time_anchor(para: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\d{1,2}:\d{2}(?::\d{2})?").expect("time re"));
    re.is_match(para)
        || para.contains("倒计时")
        || para.contains("子夜")
        || para.contains("午夜")
        || para.contains("十分钟")
        || para.contains("小时")
}

/// TIMELINE issues often cite one quote while the contradiction spans many clocks.
/// Expand to every time-anchor paragraph so local revise does not "fix one, break another".
fn expand_timeline_issue_targets(
    draft: &str,
    audit_issues: &[serde_json::Value],
    targets: &mut Vec<RevisionTarget>,
) {
    let timeline = audit_issues.iter().any(|i| {
        let ty = i.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let msg = i.get("message").and_then(|v| v.as_str()).unwrap_or("");
        ty.eq_ignore_ascii_case("TIMELINE")
            || msg.contains("时间互斥")
            || msg.contains("倒计时") && msg.contains("冲突")
            || msg.contains("钟点")
    });
    if !timeline {
        return;
    }
    let paragraphs = segment_paragraphs(draft);
    let instr = "统一本章倒计时/钟点/子夜表述，消除互斥：只保留一条单调当前读数（倒计时只减不增或明确冻结）；\
禁止数值无故回跳；回忆初始值须标明「最初」；删去多余递减串，数字出现尽量少。"
        .to_string();
    for (i, p) in paragraphs.iter().enumerate() {
        if !looks_like_time_anchor(p) {
            continue;
        }
        if targets.iter().any(|t| i >= t.start && i <= t.end) {
            continue;
        }
        targets.push(RevisionTarget {
            instruction: instr.clone(),
            start: i,
            end: i,
            quote: String::new(),
            source: "timeline_expand".into(),
        });
    }
}

fn merge_overlapping(mut targets: Vec<RevisionTarget>) -> Vec<RevisionTarget> {
    if targets.is_empty() {
        return targets;
    }
    targets.sort_by_key(|t| (t.start, t.end));
    let mut out: Vec<RevisionTarget> = Vec::new();
    for t in targets {
        if let Some(last) = out.last_mut() {
            if t.start <= last.end.saturating_add(1) {
                last.end = last.end.max(t.end);
                if !t.instruction.is_empty() {
                    last.instruction = format!("{}；{}", last.instruction, t.instruction);
                }
                continue;
            }
        }
        out.push(t);
    }
    out.truncate(8);
    out
}

fn looks_like_timeline_target(t: &RevisionTarget) -> bool {
    if t.source == "timeline_expand" || t.source == "timeline_batch" {
        return true;
    }
    let instr = t.instruction.as_str();
    instr.contains("时间互斥")
        || instr.contains("倒计时")
        || instr.contains("钟点")
        || instr.contains("子夜")
}

/// Fold every timeline-related span into **one** batch target (single LLM round-trip).
fn coalesce_timeline_batch(targets: Vec<RevisionTarget>) -> Vec<RevisionTarget> {
    let mut idxs: Vec<usize> = Vec::new();
    let mut instr = String::new();
    let mut rest = Vec::new();
    for t in targets {
        if looks_like_timeline_target(&t) {
            for i in t.start..=t.end {
                if !idxs.contains(&i) {
                    idxs.push(i);
                }
            }
            if instr.is_empty() && !t.instruction.is_empty() {
                instr = t.instruction;
            }
        } else {
            rest.push(t);
        }
    }
    if idxs.is_empty() {
        return rest;
    }
    idxs.sort_unstable();
    if instr.is_empty() {
        instr = "统一本章倒计时/钟点/子夜表述，消除互斥：只保留一条单调当前读数（倒计时只减不增或明确冻结）；\
禁止数值无故回跳；回忆初始值须标明「最初」；删去多余递减串，数字出现尽量少。"
            .into();
    }
    rest.push(RevisionTarget {
        instruction: instr,
        start: idxs[0],
        end: *idxs.last().unwrap_or(&idxs[0]),
        quote: idxs
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(","),
        source: "timeline_batch".into(),
    });
    rest.sort_by_key(|t| t.start);
    rest.truncate(8);
    rest
}

pub fn slice_span(paragraphs: &[String], start: usize, end: usize) -> String {
    let end = end.min(paragraphs.len().saturating_sub(1));
    if start >= paragraphs.len() {
        return String::new();
    }
    paragraphs[start..=end].join("\n\n")
}

pub fn apply_span_replacement(
    draft: &str,
    start: usize,
    end: usize,
    replacement: &str,
) -> Option<String> {
    let paragraphs = segment_paragraphs(draft);
    if paragraphs.is_empty() || start >= paragraphs.len() {
        return None;
    }
    let end = end.min(paragraphs.len() - 1);
    let mut new_paras = paragraphs.clone();
    let repl = replacement.trim();
    if repl.is_empty() {
        return None;
    }
    let repl_paras = segment_paragraphs(repl);
    if repl_paras.is_empty() {
        new_paras.splice(start..=end, std::iter::once(repl.to_string()));
    } else {
        new_paras.splice(start..=end, repl_paras);
    }
    Some(new_paras.join("\n\n"))
}

/// Apply multiple local patches. Failed spans are skipped; full fallback only if zero succeed.
pub fn apply_local_patches(
    draft: &str,
    targets: &[RevisionTarget],
    replacements: &[(usize, String)],
) -> PatchResult {
    let mut current = draft.to_string();
    let mut applied = Vec::new();
    let mut failed = Vec::new();

    // Apply from back to front so indices stay valid
    let mut indexed: Vec<(usize, &RevisionTarget, &str)> = replacements
        .iter()
        .filter_map(|(i, r)| targets.get(*i).map(|t| (*i, t, r.as_str())))
        .collect();
    indexed.sort_by(|a, b| b.1.start.cmp(&a.1.start));

    for (_i, target, repl) in indexed {
        let before = {
            let paras = segment_paragraphs(&current);
            slice_span(&paras, target.start, target.end)
        };
        match apply_span_replacement(&current, target.start, target.end, repl) {
            Some(next) => {
                applied.push(AppliedPatch {
                    start: target.start,
                    end: target.end,
                    before,
                    after: repl.trim().to_string(),
                    instruction: target.instruction.clone(),
                });
                current = next;
            }
            None => failed.push(format!(
                "{}: 无法应用局部补丁",
                target.para_label()
            )),
        }
    }

    // Also mark targets without replacement as failed
    for (i, t) in targets.iter().enumerate() {
        if !replacements.iter().any(|(ri, _)| *ri == i)
            && !applied
                .iter()
                .any(|a| a.start == t.start && a.end == t.end)
        {
            // only if not already applied via another path
        }
    }

    let used_local_patch = !applied.is_empty();
    PatchResult {
        draft: current,
        applied,
        failed,
        used_local_patch,
        needs_full_fallback: !used_local_patch && !targets.is_empty(),
    }
}

/// Prefer local: build targets; if empty and instructions exist without span, signal full fallback.
pub fn plan_revision(
    draft: &str,
    user_instructions: Option<&str>,
    audit_issues: &[serde_json::Value],
    pacing_suggestions: &[serde_json::Value],
) -> (Vec<RevisionTarget>, bool /* prefer_local */) {
    let targets = collect_revision_targets(
        draft,
        user_instructions,
        audit_issues,
        pacing_suggestions,
    );
    let prefer_local = !targets.is_empty();
    (targets, prefer_local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_and_replace() {
        let draft = "第一段内容。\n\n第二段内容。\n\n第三段内容。";
        let paras = segment_paragraphs(draft);
        assert_eq!(paras.len(), 3);
        let next = apply_span_replacement(draft, 1, 1, "第二段已改。").unwrap();
        assert!(next.contains("第二段已改"));
        assert!(next.contains("第一段内容"));
        assert!(next.contains("第三段内容"));
    }

    #[test]
    fn parse_para_and_user_targets() {
        assert_eq!(parse_para_range("第3段"), Some((3, 3)));
        assert_eq!(parse_para_range("第2段至第4段"), Some((2, 4)));
        assert_eq!(parse_para_range("第1—2段（开篇描写）"), Some((1, 2)));
        assert_eq!(parse_para_range("第1-2段"), Some((1, 2)));
        let draft = "甲甲甲。\n\n乙乙乙。\n\n丙丙丙。";
        let targets = targets_from_user_instructions(draft, "改第2段，写得更紧张");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].start, 1);
    }

    #[test]
    fn partial_failure_keeps_success() {
        let draft = "A段。\n\nB段。\n\nC段。";
        let targets = vec![
            RevisionTarget {
                instruction: "改A".into(),
                start: 0,
                end: 0,
                quote: String::new(),
                source: "t".into(),
            },
            RevisionTarget {
                instruction: "改坏".into(),
                start: 99,
                end: 99,
                quote: String::new(),
                source: "t".into(),
            },
        ];
        let result = apply_local_patches(
            draft,
            &targets,
            &[(0, "A改了。".into()), (1, "不会命中".into())],
        );
        assert!(result.used_local_patch);
        assert!(!result.needs_full_fallback);
        assert!(result.draft.contains("A改了"));
    }

    #[test]
    fn pacing_detail_and_quote_drive_instruction() {
        let draft = "第一段环境很长。\n\n第二段动作。";
        let suggestions = vec![serde_json::json!({
            "priority": "P1",
            "location": "第1段",
            "quote": "环境很长",
            "action": "删减",
            "detail": "删去环境两句，保留进门动作",
        })];
        let targets = collect_revision_targets(draft, None, &[], &suggestions);
        assert_eq!(targets.len(), 1);
        assert!(targets[0].instruction.contains("删去环境"));
        assert!(targets[0].instruction.contains("删减"));
        assert_eq!(targets[0].start, 0);
    }

    #[test]
    fn timeline_issue_batches_all_time_anchors() {
        let draft = "倒计时：23:26:58。\n\n普通段落。\n\n手机显示 22:17:03。\n\n又到子夜。";
        let issues = vec![serde_json::json!({
            "type": "TIMELINE",
            "priority": "P0",
            "message": "时间互斥",
            "location": "第3段",
            "quote": "22:17:03"
        })];
        let targets = collect_revision_targets(draft, None, &issues, &[]);
        assert_eq!(targets.len(), 1, "expected one timeline batch, got {targets:?}");
        assert_eq!(targets[0].source, "timeline_batch");
        let idxs = targets[0].batch_para_indices();
        assert!(
            idxs.contains(&0) && idxs.contains(&2) && idxs.contains(&3),
            "{idxs:?}"
        );
        assert!(!idxs.contains(&1), "non-anchor para must stay out: {idxs:?}");
        assert_eq!(targets[0].para_label(), "时间锚点×3");
    }
}
