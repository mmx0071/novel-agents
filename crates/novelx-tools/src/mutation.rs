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
    ToolResult {
        output: format!("⏸ 待确认：{summary}\n\n请在审批卡选择「应用修改」或「放弃」。"),
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
