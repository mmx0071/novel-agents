//! Per-issue audit decisions — compact briefs + dynamic gate options.

use novelx_harness::{issue_priority, issue_type, with_issue_ids};
use novelx_protocol::UserInputOption;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One user-facing decision bound to a concrete tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditDecisionOption {
    pub id: String,
    pub label: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

/// Compact checklist for the chat bubble (full prose stays on the tool card).
pub fn format_audit_issue_checklist(issues: &[Value], chapter: u32) -> String {
    let issues = with_issue_ids(issues.to_vec());
    let p0 = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .count();
    let p1 = issues
        .iter()
        .filter(|i| issue_priority(i) == "P1")
        .count();
    let mut lines = vec![format!(
        "## 第{chapter}章审校未通过\n\n阻断 **{p0}** · 建议 **{p1}** · 共 {} 条（全文见审校工具卡）\n\n### 问题清单",
        issues.len()
    )];
    // P0 first, then others.
    let mut ordered: Vec<&Value> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .collect();
    ordered.extend(
        issues
            .iter()
            .filter(|i| issue_priority(i) != "P0"),
    );
    for (i, issue) in ordered.iter().enumerate() {
        let pri = issue_priority(issue);
        let ty = issue_type(issue);
        let id = issue
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let msg = issue
            .get("message")
            .or_else(|| issue.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("一致性问题");
        let short: String = msg.chars().take(72).collect();
        let loc = issue
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let quote = issue.get("quote").and_then(|v| v.as_str()).unwrap_or("");
        let mut line = format!("{}. `{id}` [{pri}", i + 1);
        if !ty.is_empty() {
            line.push('/');
            line.push_str(&ty);
        }
        line.push_str(&format!("] {short}"));
        if !loc.is_empty() {
            line.push_str(&format!("（{loc}）"));
        }
        if !quote.is_empty() {
            let q: String = quote.chars().take(40).collect();
            line.push_str(&format!("\n   「{q}」"));
        }
        lines.push(line);
    }
    if lines.len() == 1 {
        lines.push("（无结构化问题条目；请展开审校工具卡查看原文）".into());
    }
    lines.join("\n")
}

pub fn audit_fail_prompt(chapter: u32, issues: &[Value], queue_active: bool) -> String {
    let p0 = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .count();
    if queue_active {
        format!("第{chapter}章未通过（{p0} 条阻断 · 审阅队列），请选择处理项：")
    } else {
        format!("第{chapter}章未通过（{p0} 条阻断），请选择处理项：")
    }
}

/// Deterministic per-P0 options (+ fix-all + accept). Queue adds skip/cancel.
pub fn build_fallback_audit_decisions(
    project: &str,
    chapter: u32,
    issues: &[Value],
    queue_active: bool,
) -> Vec<AuditDecisionOption> {
    let issues = with_issue_ids(issues.to_vec());
    let p0: Vec<&Value> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .collect();
    let mut out = Vec::new();

    for issue in p0.iter().take(6) {
        let id = issue
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("issue")
            .to_string();
        let ty = issue_type(issue);
        let msg = issue
            .get("message")
            .or_else(|| issue.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("问题");
        let short: String = msg.chars().take(28).collect();
        let label = if ty.is_empty() {
            format!("修：{short}")
        } else {
            format!("修：{ty} {short}")
        };
        out.push(AuditDecisionOption {
            id: format!("fix_{id}"),
            label,
            tool: "steer_run".into(),
            args: json!({
                "project": project,
                "chapter": chapter,
                "choice": "revise",
                "issue_ids": [id],
            }),
        });
    }

    if p0.len() > 1 {
        let all_ids: Vec<String> = p0
            .iter()
            .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();
        out.push(AuditDecisionOption {
            id: "fix_all_p0".into(),
            label: "修全部阻断项".into(),
            tool: "steer_run".into(),
            args: json!({
                "project": project,
                "chapter": chapter,
                "choice": "revise",
                "issue_ids": all_ids,
            }),
        });
    } else if p0.is_empty() && !issues.is_empty() {
        // Soft fail with only P1/P2 — still allow revise-all.
        let all_ids: Vec<String> = issues
            .iter()
            .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();
        out.push(AuditDecisionOption {
            id: "fix_all".into(),
            label: "按清单局部修订".into(),
            tool: "steer_run".into(),
            args: json!({
                "project": project,
                "chapter": chapter,
                "choice": "revise",
                "issue_ids": all_ids,
            }),
        });
    }

    out.push(AuditDecisionOption {
        id: "accept".into(),
        label: "接受并结束本轮".into(),
        tool: "steer_run".into(),
        args: json!({
            "project": project,
            "chapter": chapter,
            "choice": "accept",
        }),
    });

    if queue_active {
        out.push(AuditDecisionOption {
            id: "aq_skip".into(),
            label: "跳过，审下一章".into(),
            tool: "audit_chapters".into(),
            args: json!({
                "project": project,
                "action": "next",
            }),
        });
        out.push(AuditDecisionOption {
            id: "aq_cancel".into(),
            label: "结束审阅队列".into(),
            tool: "audit_chapters".into(),
            args: json!({
                "project": project,
                "action": "cancel",
            }),
        });
    }

    out
}

pub fn decisions_to_ui_options(decisions: &[AuditDecisionOption]) -> Vec<UserInputOption> {
    decisions
        .iter()
        .map(|d| UserInputOption {
            id: d.id.clone(),
            label: d.label.clone(),
        })
        .collect()
}

/// Resolve a user pick / typed token against dynamic decisions.
pub fn resolve_decision_pick(
    text: &str,
    decisions: &[AuditDecisionOption],
) -> Option<(String, Value)> {
    let t = text.trim();
    if t.is_empty() || decisions.is_empty() {
        return None;
    }
    // Display index 「1」/「2」
    if let Ok(n) = t.parse::<usize>() {
        if n >= 1 && n <= decisions.len() {
            let d = &decisions[n - 1];
            return Some((d.tool.clone(), d.args.clone()));
        }
    }
    for d in decisions {
        if d.id == t || d.label == t {
            return Some((d.tool.clone(), d.args.clone()));
        }
        // Prefix match on 「修：META …」labels
        if t.len() >= 4 && (d.label.contains(t) || t.contains(&d.label)) {
            return Some((d.tool.clone(), d.args.clone()));
        }
    }
    // Legacy aliases
    if matches!(t, "按审校局部修订" | "局部修订") {
        if let Some(d) = decisions.iter().find(|d| {
            d.id == "fix_all_p0"
                || d.id == "fix_all"
                || d.args.get("choice").and_then(|v| v.as_str()) == Some("revise")
        }) {
            return Some((d.tool.clone(), d.args.clone()));
        }
    }
    if matches!(t, "接受问题" | "接受" | "接受并结束本轮") {
        if let Some(d) = decisions.iter().find(|d| d.id == "accept") {
            return Some((d.tool.clone(), d.args.clone()));
        }
    }
    None
}

/// Validate Studio-authored options; only revise/accept/queue actions; issue_ids must exist.
pub fn validate_offered_decisions(
    project: &str,
    chapter: u32,
    issues: &[Value],
    raw_options: &[Value],
    queue_active: bool,
) -> Result<Vec<AuditDecisionOption>, String> {
    if raw_options.is_empty() {
        return Err("options 不能为空".into());
    }
    if raw_options.len() > 12 {
        return Err("options 最多 12 项".into());
    }
    let known: std::collections::HashSet<String> = issues
        .iter()
        .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    let mut out = Vec::new();
    for (i, opt) in raw_options.iter().enumerate() {
        let id = opt
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("opt_{}", i + 1));
        let label = opt
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("选项 {id} 缺少 label"))?
            .to_string();
        let action = opt
            .get("action")
            .or_else(|| opt.get("choice"))
            .and_then(|v| v.as_str())
            .unwrap_or("revise")
            .trim()
            .to_ascii_lowercase();
        let issue_ids: Vec<String> = opt
            .get("issue_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for iid in &issue_ids {
            if !known.is_empty() && !known.contains(iid) {
                return Err(format!("未知 issue_id：{iid}"));
            }
        }
        let instructions = opt
            .get("instructions")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let (tool, args) = match action.as_str() {
            "accept" | "接受" => (
                "steer_run".into(),
                json!({
                    "project": project,
                    "chapter": chapter,
                    "choice": "accept",
                }),
            ),
            "skip_queue" | "next" if queue_active => (
                "audit_chapters".into(),
                json!({ "project": project, "action": "next" }),
            ),
            "cancel_queue" | "cancel" if queue_active => (
                "audit_chapters".into(),
                json!({ "project": project, "action": "cancel" }),
            ),
            "revise" | "fix" | "局部修订" | "按审校局部修订" => {
                let mut args = json!({
                    "project": project,
                    "chapter": chapter,
                    "choice": "revise",
                    "issue_ids": issue_ids,
                });
                if !instructions.is_empty() {
                    args["instructions"] = json!(instructions);
                }
                ("steer_run".into(), args)
            }
            other => {
                return Err(format!(
                    "不支持的 action：{other}（允许 revise/accept/skip_queue/cancel_queue）"
                ));
            }
        };
        out.push(AuditDecisionOption {
            id,
            label,
            tool,
            args,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_skips_long_report_prose() {
        let issues = vec![json!({
            "type": "META",
            "priority": "P0",
            "message": "叙述中出现章号指称",
            "location": "第4段",
            "quote": "他走了五章的路",
        })];
        let brief = format_audit_issue_checklist(&issues, 17);
        assert!(brief.contains("问题清单"));
        assert!(brief.contains("META"));
        assert!(!brief.contains("【已核对】"));
        assert!(brief.contains("p0-meta-1") || brief.contains("`"));
    }

    #[test]
    fn fallback_has_per_p0_and_accept() {
        let issues = vec![
            json!({"type":"META","priority":"P0","message":"章号元叙述","id":"p0-meta-1"}),
            json!({"type":"TIMELINE","priority":"P0","message":"倒计时互斥","id":"p0-timeline-2"}),
        ];
        let opts = build_fallback_audit_decisions("demo", 17, &issues, false);
        assert!(opts.iter().any(|o| o.id == "fix_p0-meta-1"));
        assert!(opts.iter().any(|o| o.id == "fix_all_p0"));
        assert!(opts.iter().any(|o| o.id == "accept"));
        let (tool, args) = resolve_decision_pick("1", &opts).unwrap();
        assert_eq!(tool, "steer_run");
        assert_eq!(args["choice"], "revise");
    }
}
