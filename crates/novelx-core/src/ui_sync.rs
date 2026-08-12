//! UI timeline helpers — server-authoritative turns / approvals.

use novelx_protocol::UserInputOption;
use serde_json::{json, Value};

pub fn ui_turn_item_count(turns: &Value) -> usize {
    turns
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|t| {
                    t.get("items")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

pub fn ui_turns_weaker_than(incoming: &Value, existing: &Value) -> bool {
    let Some(inc) = incoming.as_array() else {
        return !existing.as_array().map(|a| a.is_empty()).unwrap_or(true);
    };
    let Some(ex) = existing.as_array() else {
        return false;
    };
    if ex.is_empty() {
        return false;
    }
    if inc.len() <= 1 {
        if let Some(t) = inc.first() {
            let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let items = t
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if (status == "running" || status == "awaiting") && items == 0 {
                return true;
            }
        }
    }
    let inc_items = ui_turn_item_count(incoming);
    let ex_items = ui_turn_item_count(existing);
    let inc_only_restored = !inc.is_empty()
        && inc.iter().all(|t| {
            t.get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .starts_with("restored-")
        });
    let ex_has_live = ex.iter().any(|t| {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
        !id.starts_with("restored-") && !id.is_empty()
    });
    if inc_only_restored && ex_has_live && ex_items >= inc_items {
        return true;
    }
    if ex_items >= 3 && inc_items + 2 < ex_items {
        return true;
    }
    // Client replay of a finished turn that still spins on「调用模型中」.
    if ui_all_turns_settled(existing)
        && ui_has_open_tool_calls(incoming)
        && !ui_has_open_tool_calls(existing)
    {
        return true;
    }
    // Client snapshot with only bulk-read tool cards must not wipe NovelX prose.
    let ex_prose = ui_agent_prose_chars(existing);
    let inc_prose = ui_agent_prose_chars(incoming);
    if ex_prose >= 80 && inc_prose + 40 < ex_prose {
        return true;
    }
    false
}

fn ui_agent_prose_chars(turns: &Value) -> usize {
    turns
        .as_array()
        .map(|arr| {
            arr.iter()
                .flat_map(|t| t.get("items").and_then(|v| v.as_array()).into_iter().flatten())
                .filter(|it| it.get("type").and_then(|v| v.as_str()) == Some("agent_message"))
                .map(|it| {
                    it.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.chars().count())
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

pub fn keep_only_ui_turn(turns: Value, turn_id: &str) -> Value {
    let Value::Array(arr) = turns else {
        return json!([]);
    };
    let kept: Vec<Value> = arr
        .into_iter()
        .filter(|t| t.get("id").and_then(|v| v.as_str()) == Some(turn_id))
        .map(|mut turn| {
            if let Value::Object(ref mut obj) = turn {
                obj.insert("approval".into(), Value::Null);
            }
            turn
        })
        .collect();
    Value::Array(kept)
}

pub fn strip_ui_approvals(turns: Value) -> Value {
    let stripped = strip_ui_mutation_previews(turns);
    let Value::Array(arr) = stripped else {
        return stripped;
    };
    let next: Vec<Value> = arr
        .into_iter()
        .map(|turn| {
            let Value::Object(mut obj) = turn else {
                return turn;
            };
            obj.insert("approval".into(), Value::Null);
            if obj.get("status").and_then(|v| v.as_str()) == Some("awaiting") {
                obj.insert("status".into(), json!("complete"));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(next)
}

/// Drop draft/mutation preview cards. Live previews belong only while `pending_mutation`
/// is open; otherwise they look like「待确认修改」under the wrong gate (e.g. chapter_next).
pub fn strip_ui_mutation_previews(turns: Value) -> Value {
    let Value::Array(arr) = turns else {
        return turns;
    };
    let next: Vec<Value> = arr
        .into_iter()
        .map(|turn| {
            let Value::Object(mut obj) = turn else {
                return turn;
            };
            if let Some(Value::Array(items)) = obj.get_mut("items") {
                items.retain(|it| {
                    !matches!(
                        it.get("type").and_then(|v| v.as_str()),
                        Some("mutation_preview" | "draft_patch")
                    )
                });
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(next)
}

/// Finish a turn with a NovelX summary **without wiping** tool / preview cards.
///
/// Previously this replaced the whole turn with a single agent_message, which made
/// `continue_writing` / audit process cards disappear after mutation confirm.
pub fn append_completion_ui_turn(
    turns: Value,
    turn_id: &str,
    summary: &str,
    awaiting: bool,
) -> Value {
    let mut arr = match turns {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    let status = if awaiting { "awaiting" } else { "complete" };

    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id == turn_id {
            continue;
        }
        // Seal sibling turns.
        obj.insert("approval".into(), Value::Null);
        if obj.get("status").and_then(|v| v.as_str()) != Some("aborted") {
            obj.insert("status".into(), json!("complete"));
        }
        finish_in_progress_items(obj);
    }

    let mut found = false;
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        if obj.get("id").and_then(|v| v.as_str()) != Some(turn_id) {
            continue;
        }
        found = true;
        // Keep an already-attached human gate (e.g. chapter_next「修正本章」).
        if !awaiting {
            obj.insert("approval".into(), Value::Null);
        }
        obj.insert("status".into(), json!(status));
        finish_in_progress_items(obj);
        let items = obj
            .entry("items")
            .or_insert_with(|| json!([]));
        let Some(items) = items.as_array_mut() else {
            break;
        };
        let mut updated = false;
        // Prefer the last agent bubble (intro / live status) for the closing summary.
        for item in items.iter_mut().rev() {
            let Value::Object(it) = item else { continue };
            if it.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                it.insert("text".into(), json!(summary));
                it.insert("status".into(), json!("completed"));
                updated = true;
                break;
            }
        }
        if !updated && !summary.trim().is_empty() {
            items.push(json!({
                "id": format!("item_done_{turn_id}"),
                "type": "agent_message",
                "text": summary,
                "status": "completed"
            }));
        }
        break;
    }

    if !found {
        arr.push(json!({
            "id": turn_id,
            "status": status,
            "approval": null,
            "items": [{
                "id": format!("item_done_{turn_id}"),
                "type": "agent_message",
                "text": summary,
                "status": "completed"
            }]
        }));
    }
    Value::Array(arr)
}

fn finish_in_progress_items(obj: &mut serde_json::Map<String, Value>) {
    let Some(Value::Array(items)) = obj.get_mut("items") else {
        return;
    };
    for item in items.iter_mut() {
        let Value::Object(it) = item else { continue };
        let st = it.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if st == "in_progress" || st == "inProgress" || st == "InProgress" {
            it.insert("status".into(), json!("completed"));
        }
    }
}

/// Append streamed text onto an existing tool card without renaming it.
/// Used when council steer/reaudit nests into the parent `audit_chapters` item.
pub fn append_ui_tool_output(
    turns: Value,
    turn_id: &str,
    item_id: &str,
    delta: &str,
    status: Option<&str>,
) -> Value {
    if delta.is_empty() && status.is_none() {
        return turns;
    }
    let mut arr = match turns {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        if obj.get("id").and_then(|v| v.as_str()) != Some(turn_id) {
            continue;
        }
        let Some(items) = obj.get_mut("items").and_then(|v| v.as_array_mut()) else {
            break;
        };
        for item in items.iter_mut() {
            let Value::Object(it) = item else { continue };
            if it.get("id").and_then(|v| v.as_str()) != Some(item_id) {
                continue;
            }
            if !delta.is_empty() {
                let prev = it.get("output").and_then(|v| v.as_str()).unwrap_or("");
                it.insert("output".into(), json!(format!("{prev}{delta}")));
            }
            if let Some(s) = status {
                it.insert("status".into(), json!(s));
            }
            break;
        }
        break;
    }
    Value::Array(arr)
}

/// Persist a tool card into `ui_turns` so HTTP restore keeps the process timeline.
pub fn upsert_ui_tool_call(
    turns: Value,
    turn_id: &str,
    item_id: &str,
    name: &str,
    arguments: &Value,
    output: Option<&str>,
    status: &str,
    duration_ms: Option<u64>,
) -> Value {
    let mut arr = match turns {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    let mut tool = json!({
        "id": item_id,
        "type": "tool_call",
        "name": name,
        "arguments": arguments,
        "output": output.unwrap_or(""),
        "status": status,
        "_key": format!("tool_call:{item_id}"),
    });
    if let Some(ms) = duration_ms {
        tool["duration_ms"] = json!(ms);
    }

    let mut matched = false;
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        if obj.get("id").and_then(|v| v.as_str()) != Some(turn_id) {
            continue;
        }
        matched = true;
        let tool_running = matches!(status, "in_progress" | "inProgress" | "InProgress");
        if tool_running && obj.get("status").and_then(|v| v.as_str()) == Some("complete") {
            // Keep recording mid-flight tools even if a race marked the turn done.
            obj.insert("status".into(), json!("running"));
        }
        let items = obj
            .entry("items")
            .or_insert_with(|| json!([]));
        let Some(items) = items.as_array_mut() else {
            continue;
        };
        let mut found = false;
        for item in items.iter_mut() {
            let Value::Object(it) = item else { continue };
            if it.get("id").and_then(|v| v.as_str()) == Some(item_id) {
                it.insert("type".into(), json!("tool_call"));
                it.insert("name".into(), json!(name));
                it.insert("arguments".into(), arguments.clone());
                if let Some(out) = output {
                    // Prefer longer streamed output (don't shrink on a short final line).
                    let prev = it.get("output").and_then(|v| v.as_str()).unwrap_or("");
                    if out.len() >= prev.len() || prev.trim().is_empty() {
                        it.insert("output".into(), json!(out));
                    }
                }
                it.insert("status".into(), json!(status));
                if let Some(ms) = duration_ms {
                    it.insert("duration_ms".into(), json!(ms));
                }
                found = true;
                break;
            }
        }
        if !found {
            items.push(tool.clone());
        }
        break;
    }
    if !matched {
        arr.push(json!({
            "id": turn_id,
            "status": "running",
            "approval": null,
            "items": [tool],
        }));
    }
    Value::Array(arr)
}

pub fn update_ui_turn_summary(turns: Value, turn_id: &str, summary: &str) -> Value {
    let Value::Array(mut arr) = turns else {
        return turns;
    };
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        if obj.get("id").and_then(|v| v.as_str()) != Some(turn_id) {
            continue;
        }
        if let Some(Value::Array(items)) = obj.get_mut("items") {
            // Only the first agent bubble (status / intro). Later bubbles — e.g. audit
            // report from emit_audit_report_before_gate — must not be overwritten.
            for item in items.iter_mut() {
                let Value::Object(it) = item else { continue };
                if it.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                    it.insert("text".into(), json!(summary));
                    it.insert("status".into(), json!("completed"));
                    break;
                }
            }
        }
    }
    Value::Array(arr)
}

/// Ensure a NovelX prose bubble exists on `turn_id` (match by item id, else append).
/// Used so HTTP restore / client turn sync cannot leave only bulk-read tool cards.
pub fn upsert_ui_agent_message(
    turns: Value,
    turn_id: &str,
    item_id: &str,
    text: &str,
) -> Value {
    let text = text.trim();
    if text.is_empty() {
        return turns;
    }
    let Value::Array(mut arr) = turns else {
        return turns;
    };
    let mut matched_turn = false;
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        if obj.get("id").and_then(|v| v.as_str()) != Some(turn_id) {
            continue;
        }
        matched_turn = true;
        let items = obj
            .entry("items")
            .or_insert_with(|| json!([]));
        let Some(items) = items.as_array_mut() else {
            continue;
        };
        let mut found = false;
        for item in items.iter_mut() {
            let Value::Object(it) = item else { continue };
            if it.get("id").and_then(|v| v.as_str()) == Some(item_id)
                || (it.get("type").and_then(|v| v.as_str()) == Some("agent_message")
                    && it
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                    && it.get("id").and_then(|v| v.as_str()) == Some(item_id))
            {
                it.insert("type".into(), json!("agent_message"));
                it.insert("text".into(), json!(text));
                it.insert("status".into(), json!("completed"));
                found = true;
                break;
            }
        }
        if !found {
            // Prefer updating the last empty agent_message; else append after tools.
            if let Some(item) = items.iter_mut().rev().find(|item| {
                item.get("type").and_then(|v| v.as_str()) == Some("agent_message")
                    && item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .is_empty()
            }) {
                if let Value::Object(it) = item {
                    it.insert("id".into(), json!(item_id));
                    it.insert("text".into(), json!(text));
                    it.insert("status".into(), json!("completed"));
                }
            } else {
                items.push(json!({
                    "id": item_id,
                    "type": "agent_message",
                    "text": text,
                    "status": "completed",
                }));
            }
        }
    }
    if !matched_turn {
        arr.push(json!({
            "id": turn_id,
            "status": "complete",
            "approval": null,
            "items": [{
                "id": item_id,
                "type": "agent_message",
                "text": text,
                "status": "completed",
            }],
        }));
    }
    Value::Array(arr)
}

/// Attach mutation preview items (diffs / markdown) onto the turn for Web cards.
pub fn attach_ui_mutation_preview(
    turns: Value,
    turn_id: &str,
    preview: &Value,
    diffs: Value,
) -> Value {
    let mut arr = match turns {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    let mut items: Vec<Value> = Vec::new();
    let chapter = preview
        .get("chapter")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let kind = preview
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Paragraph body patches only. Document / outline mutations keep full text for
    // writing-desk inline −/+ (even when `chapter` is set, e.g. revise_outline).
    let body_para_patch = chapter > 0
        && matches!(
            kind,
            "revise_chapter" | "split_chapter" | "apply_draft_patch" | "draft_patch" | ""
        )
        && !preview.get("before_full").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())
        && !preview.get("after_full").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
    if let Some(darr) = diffs.as_array() {
        if body_para_patch {
            for d in darr {
                let start = d.get("start_para").and_then(|v| v.as_u64()).unwrap_or(1);
                let end = d
                    .get("end_para")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(start);
                items.push(json!({
                    "type": "draft_patch",
                    "status": "completed",
                    "project": preview.get("project").and_then(|v| v.as_str()).unwrap_or(""),
                    "chapter": chapter,
                    "start_para": start,
                    "end_para": end,
                    "before": d.get("before").and_then(|v| v.as_str()).unwrap_or(""),
                    "after": d.get("after").and_then(|v| v.as_str()).unwrap_or(""),
                    "readerTab": "draft",
                }));
            }
        } else if !darr.is_empty()
            || preview
                .get("before_full")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || preview
                .get("after_full")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            let hunks: Vec<Value> = darr
                .iter()
                .map(|d| {
                    json!({
                        "before": d.get("before").and_then(|v| v.as_str()).unwrap_or(""),
                        "after": d.get("after").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect();
            items.push(json!({
                "type": "mutation_preview",
                "status": "completed",
                "kind": kind,
                "topic": preview.get("topic").and_then(|v| v.as_str()).unwrap_or(""),
                "path": preview
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                "title": preview.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "name": preview.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                "markdown": preview.get("markdown").and_then(|v| v.as_str()).unwrap_or(""),
                "before_full": preview.get("before_full").and_then(|v| v.as_str()).unwrap_or(""),
                "after_full": preview.get("after_full").and_then(|v| v.as_str()).unwrap_or(""),
                "chapter": chapter,
                "fields": preview.get("fields").cloned().unwrap_or(Value::Null),
                "diffs": hunks,
            }));
        }
    }
    if items.is_empty() {
        let markdown = preview
            .get("markdown")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fields = preview.get("fields").cloned().unwrap_or(Value::Null);
        let before_full = preview
            .get("before_full")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let after_full = preview
            .get("after_full")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !markdown.is_empty() || !fields.is_null() || !before_full.is_empty() || !after_full.is_empty()
        {
            items.push(json!({
                "type": "mutation_preview",
                "status": "completed",
                "kind": kind,
                "topic": preview.get("topic").and_then(|v| v.as_str()).unwrap_or(""),
                "path": preview
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                "markdown": markdown,
                "before_full": before_full,
                "after_full": after_full,
                "chapter": chapter,
                "fields": fields,
            }));
        }
    }
    if items.is_empty() {
        return Value::Array(arr);
    }
    let push_items = |obj: &mut serde_json::Map<String, Value>| {
        let mut existing = obj
            .get("items")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // sync_pending_gate / maybe_offer may both attach — don't duplicate cards.
        let has_preview = existing.iter().any(|it| {
            matches!(
                it.get("type").and_then(|v| v.as_str()),
                Some("mutation_preview" | "draft_patch")
            )
        });
        if has_preview {
            return;
        }
        existing.extend(items.clone());
        obj.insert("items".into(), Value::Array(existing));
    };
    let mut matched = false;
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id == turn_id {
            push_items(obj);
            matched = true;
            break;
        }
    }
    if !matched {
        if let Some(Value::Object(obj)) = arr.last_mut() {
            push_items(obj);
        } else {
            arr.push(json!({
                "id": turn_id,
                "status": "awaiting",
                "items": items,
            }));
        }
    }
    Value::Array(arr)
}

pub fn attach_ui_approval(
    turns: Value,
    turn_id: &str,
    prompt: &str,
    options: &[UserInputOption],
) -> Value {
    let gate = json!({
        "prompt": prompt,
        "options": options,
    });
    let mut arr = match turns {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    if arr.is_empty() {
        arr.push(json!({
            "id": turn_id,
            "status": "awaiting",
            "items": [],
            "approval": gate,
        }));
        return Value::Array(arr);
    }
    let mut matched = false;
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id == turn_id {
            obj.insert("approval".into(), gate.clone());
            if obj.get("status").and_then(|v| v.as_str()) != Some("complete") {
                obj.insert("status".into(), json!("awaiting"));
            }
            matched = true;
        } else {
            obj.insert("approval".into(), Value::Null);
        }
    }
    if !matched {
        if let Some(Value::Object(obj)) = arr.last_mut() {
            obj.insert("approval".into(), gate);
            if obj.get("status").and_then(|v| v.as_str()) != Some("complete") {
                obj.insert("status".into(), json!("awaiting"));
            }
        }
    }
    Value::Array(arr)
}

/// Mark in-progress items completed; running turns without approval → complete.
/// Used when the server turn ends but the client may have missed ItemCompleted.
pub fn finish_stale_ui_turns(turns: Value) -> Value {
    let Value::Array(arr) = turns else {
        return turns;
    };
    let next: Vec<Value> = arr
        .into_iter()
        .map(|turn| {
            let Value::Object(mut obj) = turn else {
                return turn;
            };
            if let Some(Value::Array(items)) = obj.get_mut("items") {
                for item in items.iter_mut() {
                    let Some(it) = item.as_object_mut() else {
                        continue;
                    };
                    let st = it.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if st == "in_progress" || st == "inProgress" || st == "InProgress" || st.is_empty()
                    {
                        let ty = it.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if matches!(
                            ty,
                            "tool_call" | "agent_message" | "skill_load" | "pipeline_step" | "reasoning"
                        ) {
                            it.insert("status".into(), json!("completed"));
                        }
                    }
                }
            }
            let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let has_approval = obj
                .get("approval")
                .map(|a| !a.is_null())
                .unwrap_or(false);
            if status == "running" && !has_approval {
                obj.insert("status".into(), json!("complete"));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(next)
}

pub fn mark_ui_turn_complete(turns: Value, turn_id: &str) -> Value {
    let Value::Array(arr) = turns else {
        return turns;
    };
    let next: Vec<Value> = arr
        .into_iter()
        .map(|turn| {
            let Value::Object(mut obj) = turn else {
                return turn;
            };
            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let matched = id == turn_id;
            if matched || obj.get("status").and_then(|v| v.as_str()) == Some("running") {
                if let Some(Value::Array(items)) = obj.get_mut("items") {
                    for item in items.iter_mut() {
                        let Some(it) = item.as_object_mut() else {
                            continue;
                        };
                        let st = it.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        if st == "in_progress" || st == "inProgress" || st == "InProgress" || st.is_empty()
                        {
                            it.insert("status".into(), json!("completed"));
                        }
                    }
                }
                if matched {
                    let has_approval = obj
                        .get("approval")
                        .map(|a| !a.is_null())
                        .unwrap_or(false);
                    if !has_approval {
                        obj.insert("status".into(), json!("complete"));
                    }
                }
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(next)
}

fn ui_has_open_tool_calls(turns: &Value) -> bool {
    turns
        .as_array()
        .map(|arr| {
            arr.iter().any(|t| {
                t.get("items")
                    .and_then(|v| v.as_array())
                    .map(|items| {
                        items.iter().any(|it| {
                            it.get("type").and_then(|v| v.as_str()) == Some("tool_call")
                                && matches!(
                                    it.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                                    "in_progress" | "inProgress" | "InProgress" | ""
                                )
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn ui_all_turns_settled(turns: &Value) -> bool {
    turns
        .as_array()
        .map(|arr| {
            !arr.is_empty()
                && arr.iter().all(|t| {
                    matches!(
                        t.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                        "complete" | "awaiting" | "aborted"
                    )
                })
        })
        .unwrap_or(false)
}

pub fn sanitize_ui_turns_finish_audits(turns: Value) -> Value {
    let Value::Array(arr) = turns else {
        return turns;
    };
    let next: Vec<Value> = arr
        .into_iter()
        .map(|turn| {
            let Value::Object(mut obj) = turn else {
                return turn;
            };
            if let Some(Value::Array(items)) = obj.get_mut("items") {
                for item in items.iter_mut() {
                    let Some(obj) = item.as_object_mut() else {
                        continue;
                    };
                    let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    let auditish = ty == "tool_call"
                        && (name == "audit_chapter"
                            || name == "audit_chapters"
                            || name == "steer_run");
                    if auditish
                        && (status == "in_progress" || status == "inProgress" || status.is_empty())
                    {
                        obj.insert("status".into(), json!("completed"));
                    }
                }
            }
            if obj.get("status").and_then(|v| v.as_str()) == Some("running") {
                obj.insert("status".into(), json!("awaiting"));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_completion_preserves_tool_cards() {
        let turns = json!([{
            "id": "turn_1",
            "status": "running",
            "items": [
                {"id": "a1", "type": "agent_message", "text": "已确认，正在应用…", "status": "completed"},
                {
                    "id": "t1",
                    "type": "tool_call",
                    "name": "continue_writing",
                    "arguments": {"project": "sample-novel"},
                    "output": "▶ 章纲规划\n✓ 正文写作",
                    "status": "completed"
                }
            ]
        }]);
        let next = append_completion_ui_turn(turns, "turn_1", "已确认…\n\n完成，但有警告", false);
        let items = next[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "tool card must survive completion");
        assert_eq!(items[1]["type"], "tool_call");
        assert_eq!(items[1]["name"], "continue_writing");
        assert!(items[0]["text"].as_str().unwrap().contains("完成"));
        assert_eq!(next[0]["status"], "complete");
    }

    #[test]
    fn append_ui_tool_output_keeps_parent_name_and_running() {
        let turns = json!([{
            "id": "turn_1",
            "status": "running",
            "items": [{
                "id": "audit_1",
                "type": "tool_call",
                "name": "audit_chapters",
                "arguments": {"project": "sample-novel", "from": 1, "to": 3},
                "output": "审阅队列：第1章",
                "status": "in_progress"
            }]
        }]);
        let next = append_ui_tool_output(
            turns,
            "turn_1",
            "audit_1",
            "\n评审团自动修订中…",
            Some("in_progress"),
        );
        let item = &next[0]["items"][0];
        assert_eq!(item["name"], "audit_chapters");
        assert_eq!(item["status"], "in_progress");
        let out = item["output"].as_str().unwrap();
        assert!(out.contains("审阅队列：第1章"));
        assert!(out.contains("评审团自动修订中"));
    }

    #[test]
    fn strip_ui_mutation_previews_removes_preview_cards() {
        let turns = json!([{
            "id": "turn_1",
            "status": "awaiting",
            "approval": {"prompt": "next", "options": [{"id": "cn_continue", "label": "继续创作"}]},
            "items": [
                {"type": "agent_message", "text": "ok"},
                {"type": "mutation_preview", "kind": "upsert_setting", "markdown": "x"},
                {"type": "draft_patch", "before": "a", "after": "b"},
                {"type": "tool_call", "name": "audit_chapter"}
            ]
        }]);
        let next = strip_ui_mutation_previews(turns);
        let items = next[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["type"], "agent_message");
        assert_eq!(items[1]["type"], "tool_call");
        assert!(!next[0]["approval"].is_null());
    }

    #[test]
    fn mutation_preview_attach_is_idempotent() {
        let turns = json!([{
            "id": "turn_1",
            "status": "awaiting",
            "items": []
        }]);
        let preview = json!({
            "kind": "continue_writing",
            "markdown": "确认后开始写第1章",
            "fields": null
        });
        let once = attach_ui_mutation_preview(turns, "turn_1", &preview, json!([]));
        let twice = attach_ui_mutation_preview(once, "turn_1", &preview, json!([]));
        let items = twice[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "preview must not duplicate");
        assert_eq!(items[0]["type"], "mutation_preview");
    }

    #[test]
    fn document_mutation_diffs_become_mutation_preview_hunks() {
        let turns = json!([{
            "id": "turn_1",
            "status": "awaiting",
            "items": []
        }]);
        let preview = json!({
            "kind": "upsert_setting",
            "topic": "完整世界观",
            "path": "artifacts/bible.md",
            "markdown": "节选",
            "before_full": "历史事件。\n",
            "after_full": "历史事件，如曼德拉去世。\n"
        });
        let diffs = json!([{
            "before": "历史事件。",
            "after": "历史事件，如曼德拉去世。"
        }]);
        let next = attach_ui_mutation_preview(turns, "turn_1", &preview, diffs);
        let items = next[0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "mutation_preview");
        assert_eq!(items[0]["diffs"][0]["after"], "历史事件，如曼德拉去世。");
        assert_eq!(items[0]["before_full"], "历史事件。\n");
        assert_eq!(items[0]["after_full"], "历史事件，如曼德拉去世。\n");
        assert!(items[0].get("chapter").is_none() || items[0]["chapter"] == 0);
    }

    #[test]
    fn revise_outline_keeps_mutation_preview_even_with_chapter() {
        let turns = json!([{
            "id": "turn_1",
            "status": "awaiting",
            "items": []
        }]);
        let preview = json!({
            "kind": "revise_outline",
            "chapter": 2,
            "before_full": "旧纲",
            "after_full": "新纲",
            "path": "chapters/002/outline.json"
        });
        let diffs = json!([{ "before": "旧纲", "after": "新纲" }]);
        let next = attach_ui_mutation_preview(turns, "turn_1", &preview, diffs);
        let items = next[0]["items"].as_array().unwrap();
        assert_eq!(items[0]["type"], "mutation_preview");
        assert_eq!(items[0]["after_full"], "新纲");
    }

    #[test]
    fn chapter_diffs_still_attach_as_draft_patch() {
        let turns = json!([{
            "id": "turn_1",
            "status": "awaiting",
            "items": []
        }]);
        let preview = json!({
            "kind": "revise_chapter",
            "chapter": 3,
            "project": "sample-novel"
        });
        let diffs = json!([{
            "before": "旧句",
            "after": "新句",
            "start_para": 2,
            "end_para": 2
        }]);
        let next = attach_ui_mutation_preview(turns, "turn_1", &preview, diffs);
        let items = next[0]["items"].as_array().unwrap();
        assert_eq!(items[0]["type"], "draft_patch");
        assert_eq!(items[0]["chapter"], 3);
        assert_eq!(items[0]["before"], "旧句");
    }
}
