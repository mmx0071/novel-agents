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
    is_routine_self_confirm(config_root, kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
}
