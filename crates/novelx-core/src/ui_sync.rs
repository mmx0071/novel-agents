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
    false
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
    let Value::Array(arr) = turns else {
        return turns;
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
    for turn in &mut arr {
        let Value::Object(obj) = turn else { continue };
        obj.insert("approval".into(), Value::Null);
        obj.insert("status".into(), json!("complete"));
        if let Some(Value::Array(items)) = obj.get_mut("items") {
            for item in items.iter_mut() {
                let Value::Object(it) = item else { continue };
                let st = it.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if st == "in_progress" || st == "inProgress" || st == "InProgress" {
                    it.insert("status".into(), json!("completed"));
                }
            }
        }
    }
    arr.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(turn_id));
    arr.push(json!({
        "id": turn_id,
        "status": if awaiting { "awaiting" } else { "complete" },
        "approval": null,
        "items": [{
            "id": format!("item_done_{turn_id}"),
            "type": "agent_message",
            "text": summary,
            "status": "completed"
        }]
    }));
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
            for item in items.iter_mut() {
                let Value::Object(it) = item else { continue };
                if it.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                    it.insert("text".into(), json!(summary));
                    it.insert("status".into(), json!("completed"));
                }
            }
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
