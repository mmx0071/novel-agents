//! Mutation confirm helpers: preview first, apply only with `apply=true`.

use crate::ToolResult;
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

/// Whether tools must preview before disk writes.
pub fn mutation_confirm_enabled(config_root: &Path) -> bool {
    novelx_pipeline::PhaseEnforceFlags::load(config_root).mutation_confirm
}

pub fn wants_apply(args: &Value) -> bool {
    args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Human gate already confirmed intent (e.g. chapter_order → continue_writing).
/// Skips a second mutation confirm card; still subject to hard gates.
pub fn confirm_skipped(args: &Value) -> bool {
    args.get("confirm_skip")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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
            "⏸ 修订预览：{summary}\n\n请对照原文与修订（写作台「修订对照」或聊天 diff 卡），确认后选「应用修改」落盘，或「放弃」。"
        )
    } else {
        format!("⏸ 待确认：{summary}\n\n请查看预览后选「应用修改」落盘，或「放弃」。")
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
/// `apply=true` without `mutation_id` is rejected (cannot skip the confirm card).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_with_diffs_mentions_desk_diff() {
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
        assert!(r.output.contains("修订对照") || r.output.contains("diff"));
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
        assert!(!r.output.contains("写作台「修订对照」"));
    }
}
