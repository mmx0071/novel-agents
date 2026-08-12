//! Mutation confirm helpers: preview first, apply only with `apply=true`.
//! With `studio.mutation_severity_policy`, routine tools self-confirm (no human card).

use crate::mutation_policy::{cached_policy, severity_policy_enabled};
use crate::ToolResult;
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

/// Whether tools must preview before disk writes.
pub fn mutation_confirm_enabled(config_root: &Path) -> bool {
    novelx_pipeline::PhaseEnforceFlags::load(config_root).mutation_confirm
}

/// `studio.agent_auto_apply_mutations` — Agent may skip human「应用修改」cards.
/// Read from disk each call so ConfigPanel toggles apply without process restart.
pub fn agent_auto_apply_enabled(config_root: &Path) -> bool {
    feature_flag(config_root, "studio.agent_auto_apply_mutations", false)
}

fn feature_flag(config_root: &Path, key: &str, default: bool) -> bool {
    let path = config_root.join("features.yaml");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return default;
    };
    #[derive(serde::Deserialize)]
    struct FeaturesFile {
        #[serde(default)]
        features: std::collections::HashMap<String, bool>,
    }
    serde_yaml::from_str::<FeaturesFile>(&raw)
        .ok()
        .and_then(|f| f.features.get(key).copied())
        .unwrap_or(default)
}

/// Irreversible / restore ops always need a human card, even with agent auto-apply.
pub fn always_require_human_confirm(kind: &str) -> bool {
    matches!(
        kind,
        "delete_entity" | "restore_version_node"
    )
}

/// True when this mutation kind should self-confirm (no human card).
pub fn agent_may_auto_apply(config_root: &Path, kind: &str) -> bool {
    agent_auto_apply_enabled(config_root) && !always_require_human_confirm(kind)
}

pub fn wants_apply(args: &Value) -> bool {
    args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Bool from JSON bool or gate-template string `"true"` / `"1"`.
pub fn json_bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| {
        v.as_bool().or_else(|| {
            v.as_str()
                .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
        })
    })
}

/// Human gate already confirmed intent (e.g. chapter_order → continue_writing).
/// Skips a second mutation confirm card; still subject to hard gates.
pub fn confirm_skipped(args: &Value) -> bool {
    args.get("confirm_skip")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Routine tool under severity policy — system self-confirms (no human card).
pub fn is_routine_self_confirm(config_root: &Path, kind: &str) -> bool {
    if !mutation_confirm_enabled(config_root) {
        return false;
    }
    if !severity_policy_enabled(config_root) {
        return false;
    }
    let policy = cached_policy(config_root);
    policy.severity_mode && policy.is_routine(kind)
}

/// When confirm is required, `apply=true` must carry a `mutation_id` from a prior preview.
pub fn apply_without_mutation_id(config_root: &Path, args: &Value) -> bool {
    if confirm_skipped(args) {
        return false;
    }
    mutation_confirm_enabled(config_root)
        && wants_apply(args)
        && args
            .get("mutation_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
}

pub fn reject_apply_without_id() -> ToolResult {
    ToolResult {
        output: "⛔ 落盘被拒绝：须先预览并由用户确认（缺少 mutation_id）。".into(),
        data: json!({
            "blocked": true,
            "reason": "mutation_id_required",
        }),
    }
}

pub fn new_mutation_id() -> String {
    format!("mut_{}", Uuid::new_v4().simple())
}

fn preview_has_text_diffs(preview: &Value) -> bool {
    preview
        .get("diffs")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty())
}

/// Max lines kept on each side of a hunk (keeps confirm cards readable).
const HUNK_LINE_CAP: usize = 48;

/// Attach `diffs` + full documents so the writing desk can render inline −/+ at real positions.
pub fn with_text_diff(mut preview: Value, before: &str, after: &str) -> Value {
    let diffs = text_hunk_diffs(before, after);
    if let Some(obj) = preview.as_object_mut() {
        obj.insert("diffs".into(), diffs);
        obj.insert("before_full".into(), Value::String(before.to_string()));
        obj.insert("after_full".into(), Value::String(after.to_string()));
    }
    preview
}

/// Git-style line hunks for mutation confirm: `{ before, after }` per changed region.
/// Strips common prefix/suffix lines; empty before → pure additions (green +).
pub fn text_hunk_diffs(before: &str, after: &str) -> Value {
    if before == after {
        return json!([]);
    }
    let bl: Vec<&str> = before.lines().collect();
    let al: Vec<&str> = after.lines().collect();
    let mut pre = 0usize;
    while pre < bl.len() && pre < al.len() && bl[pre] == al[pre] {
        pre += 1;
    }
    let mut suf = 0usize;
    while suf < bl.len().saturating_sub(pre)
        && suf < al.len().saturating_sub(pre)
        && bl[bl.len() - 1 - suf] == al[al.len() - 1 - suf]
    {
        suf += 1;
    }
    // Only the changed mid slice — do NOT pad shared context into before/after.
    // Context lines in both sides were rendered as false −/+ pairs in chat cards.
    let b_changed_end = bl.len() - suf;
    let a_changed_end = al.len() - suf;
    let b_slice = if pre < b_changed_end {
        &bl[pre..b_changed_end]
    } else {
        &[][..]
    };
    let a_slice = if pre < a_changed_end {
        &al[pre..a_changed_end]
    } else {
        &[][..]
    };
    let (b_text, b_trunc) = join_capped(b_slice, HUNK_LINE_CAP);
    let (a_text, a_trunc) = join_capped(a_slice, HUNK_LINE_CAP);
    let mut before_out = b_text;
    let mut after_out = a_text;
    if b_trunc {
        before_out.push_str("\n…（后续删减已省略）");
    }
    if a_trunc {
        after_out.push_str("\n…（后续新增已省略）");
    }
    json!([{
        "before": before_out,
        "after": after_out,
        "start_para": 1,
        "end_para": 1,
    }])
}

fn join_capped(lines: &[&str], cap: usize) -> (String, bool) {
    if lines.is_empty() {
        return (String::new(), false);
    }
    if lines.len() <= cap {
        return (lines.join("\n"), false);
    }
    (lines[..cap].join("\n"), true)
}

/// Build a confirm-required tool result (no disk write).
pub fn preview_mutation(
    kind: &str,
    summary: &str,
    preview: Value,
    apply_tool: &str,
    mut apply_args: Value,
) -> ToolResult {
    let mutation_id = new_mutation_id();
    if let Some(obj) = apply_args.as_object_mut() {
        obj.insert("apply".into(), Value::Bool(true));
        obj.insert("mutation_id".into(), Value::String(mutation_id.clone()));
    }
    let output = if preview_has_text_diffs(&preview) {
        format!(
            "⏸ 修订预览：{summary}\n\n\
             绿 + 为新增，红 − 为删减（同 git）。选「应用修改」写入磁盘，「取消变更」丢弃本次预览。"
        )
    } else {
        format!(
            "⏸ 待确认：{summary}\n\n请查看预览后选「应用修改」落盘，或「取消变更」退回。"
        )
    };
    ToolResult {
        output,
        data: json!({
            "needs_confirm": true,
            "mutation_id": mutation_id,
            "mutation_kind": kind,
            "summary": summary,
            "preview": preview,
            "apply_tool": apply_tool,
            "apply_args": apply_args,
        }),
    }
}

/// When confirm is on and caller did not pass apply=true, return preview instead of writing.
/// Routine tools under severity policy skip the card (self-confirm).
/// `apply=true` without `mutation_id` is rejected for high tools.
/// `confirm_skip=true` (gate-sourced) proceeds without a second card.
pub fn maybe_preview(
    config_root: &Path,
    args: &Value,
    kind: &str,
    summary: &str,
    preview: Value,
    apply_tool: &str,
    apply_args: Value,
) -> Option<ToolResult> {
    if !mutation_confirm_enabled(config_root) {
        return None;
    }
    if confirm_skipped(args) {
        return None;
    }
    // 1C: routine tools self-confirm — write immediately without human card.
    if is_routine_self_confirm(config_root, kind) {
        return None;
    }
    // Agent auto-apply (连写/无人值守)：可自行落盘，跳过「应用修改」卡。
    if agent_may_auto_apply(config_root, kind) {
        return None;
    }
    if apply_without_mutation_id(config_root, args) {
        return Some(reject_apply_without_id());
    }
    if wants_apply(args) {
        return None;
    }
    Some(preview_mutation(kind, summary, preview, apply_tool, apply_args))
}

/// Mark tool args as already confirmed by a human gate (no second mutation card).
pub fn with_confirm_skip(mut args: Value) -> Value {
    if let Some(obj) = args.as_object_mut() {
        obj.insert("confirm_skip".into(), Value::Bool(true));
    }
    args
}

/// True when this tool call will mutate disk without opening a preview card.
pub fn will_write_without_preview(config_root: &Path, kind: &str, args: &Value) -> bool {
    if confirm_skipped(args) || wants_apply(args) {
        return true;
    }
    if !mutation_confirm_enabled(config_root) {
        return true;
    }
    if agent_may_auto_apply(config_root, kind) {
        return true;
    }
    is_routine_self_confirm(config_root, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn preview_with_diffs_mentions_git_style() {
        let r = preview_mutation(
            "revise_local",
            "第1章局部修订（1 处）",
            json!({
                "diffs": [{ "before": "a", "after": "b" }],
            }),
            "revise_chapter",
            json!({ "project": "sample-novel", "chapter": 1 }),
        );
        assert!(r.output.contains("修订预览"));
        assert!(r.output.contains("绿 +") || r.output.contains("git"));
        assert!(r.output.contains("取消变更"));
        assert!(!r.output.starts_with("⏸ 待确认："));
    }

    #[test]
    fn preview_without_diffs_uses_generic_confirm_copy() {
        let r = preview_mutation(
            "update_plot",
            "更新剧情卡",
            json!({ "fields": { "status": "active" } }),
            "update_plot",
            json!({ "project": "sample-novel" }),
        );
        assert!(r.output.starts_with("⏸ 待确认："));
        assert!(r.output.contains("取消变更"));
    }

    #[test]
    fn text_hunk_diffs_highlights_changed_middle() {
        let before = "a\nb\nold line\nc\n";
        let after = "a\nb\nnew line with 如曼德拉去世\nc\n";
        let diffs = text_hunk_diffs(before, after);
        let arr = diffs.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let b = arr[0]["before"].as_str().unwrap();
        let a = arr[0]["after"].as_str().unwrap();
        assert!(b.contains("old line"), "{b}");
        assert!(a.contains("如曼德拉去世"), "{a}");
        assert!(!b.contains("如曼德拉去世"), "{b}");
    }

    #[test]
    fn text_hunk_diffs_empty_before_is_addition() {
        let diffs = text_hunk_diffs("", "# 世界观\n\n## 0. x\n");
        let arr = diffs.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["before"].as_str().unwrap(), "");
        assert!(arr[0]["after"].as_str().unwrap().contains("世界观"));
    }

    #[test]
    fn text_hunk_diffs_pure_mid_insert_has_no_false_context_minus() {
        let before = "a\nb\nc\n";
        let after = "a\nb\nNEW section\nc\n";
        let diffs = text_hunk_diffs(before, after);
        let arr = diffs.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        // Must not put shared context line "b" into before (was rendered as false −).
        assert_eq!(arr[0]["before"].as_str().unwrap(), "");
        assert_eq!(arr[0]["after"].as_str().unwrap(), "NEW section");
    }

    #[test]
    fn routine_skips_preview_when_severity_on() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-mut-sev-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut feat = std::fs::File::create(dir.join("features.yaml")).unwrap();
        write!(
            feat,
            "version: 1\nfeatures:\n  studio.require_mutation_confirm: true\n  studio.mutation_severity_policy: true\n"
        )
        .unwrap();
        let mut pol = std::fs::File::create(dir.join("mutation_policy.yaml")).unwrap();
        write!(
            pol,
            "version: 1\nmode: severity\nhigh_tools: [delete_entity]\nroutine_tools: [continue_writing, update_plot]\n"
        )
        .unwrap();
        let args = json!({});
        assert!(maybe_preview(
            &dir,
            &args,
            "continue_writing",
            "写章",
            json!({}),
            "continue_writing",
            json!({"project": "demo"}),
        )
        .is_none());
        assert!(maybe_preview(
            &dir,
            &args,
            "delete_entity",
            "删卡",
            json!({}),
            "delete_entity",
            json!({"project": "demo"}),
        )
        .is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_auto_apply_skips_content_preview_keeps_delete() {
        let dir = std::env::temp_dir().join(format!(
            "novelx-mut-auto-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut feat = std::fs::File::create(dir.join("features.yaml")).unwrap();
        write!(
            feat,
            "version: 1\nfeatures:\n  studio.require_mutation_confirm: true\n  studio.mutation_severity_policy: true\n  studio.agent_auto_apply_mutations: true\n"
        )
        .unwrap();
        let mut pol = std::fs::File::create(dir.join("mutation_policy.yaml")).unwrap();
        write!(
            pol,
            "version: 1\nmode: severity\nhigh_tools: [upsert_setting, delete_entity, revise_chapter]\nroutine_tools: [continue_writing]\n"
        )
        .unwrap();
        let args = json!({});
        assert!(agent_auto_apply_enabled(&dir));
        assert!(maybe_preview(
            &dir,
            &args,
            "upsert_setting",
            "Bible",
            json!({"diffs":[{"before":"a","after":"b"}]}),
            "upsert_setting",
            json!({"project": "demo"}),
        )
        .is_none());
        assert!(maybe_preview(
            &dir,
            &args,
            "revise_chapter",
            "修订",
            json!({}),
            "revise_chapter",
            json!({"project": "demo", "chapter": 1}),
        )
        .is_none());
        assert!(maybe_preview(
            &dir,
            &args,
            "delete_entity",
            "删卡",
            json!({}),
            "delete_entity",
            json!({"project": "demo"}),
        )
        .is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
