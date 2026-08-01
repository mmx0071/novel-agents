use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::review_priority::{
    has_timeline_p0, issue_priority, issue_type, partition_issues, with_issue_ids,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    Continue,
    AutoFix,
    /// Also accepts legacy/unknown wire values (e.g. removed `block`).
    #[serde(other)]
    AwaitHuman,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyGateResult {
    pub decision: GateDecision,
    pub auto_fix_issues: Vec<Value>,
    pub message: String,
}

pub fn on_consistency_result(passed: bool, issues: &[Value], continuous: bool) -> ConsistencyGateResult {
    let (auto, rest) = partition_issues(issues);
    // Passed + no P0/P1 auto-fixables: Continue even if advisory P2 remain.
    // Previously P2-only fell through to AwaitHuman and blocked unattended publish.
    if passed && auto.is_empty() {
        return ConsistencyGateResult {
            decision: GateDecision::Continue,
            auto_fix_issues: vec![],
            message: if rest.is_empty() {
                "一致性审计通过".into()
            } else {
                format!("一致性审计通过（另有 {} 条非阻断建议）", rest.len())
            },
        };
    }
    if !auto.is_empty() && continuous {
        let n = auto.len();
        let message = if has_timeline_p0(issues) {
            format!("发现 {n} 条 P0/P1 问题；含时间线 P0，将整章修订以统一倒计时/钟点")
        } else {
            format!("发现 {n} 条 P0/P1 问题，将局部自动修复")
        };
        return ConsistencyGateResult {
            decision: GateDecision::AutoFix,
            auto_fix_issues: auto,
            message,
        };
    }
    let n_auto = auto.len();
    let n_rest = rest.len();
    ConsistencyGateResult {
        decision: GateDecision::AwaitHuman,
        auto_fix_issues: auto,
        message: format!("一致性审计待确认：自动可修 {n_auto}，其余 {n_rest}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacingGateResult {
    pub decision: GateDecision,
    pub auto_fix_suggestions: Vec<Value>,
    pub message: String,
}

pub fn on_pacing_result(suggestions: &[Value], continuous: bool) -> PacingGateResult {
    let (auto, rest) = partition_issues(suggestions);
    if suggestions.is_empty() {
        return PacingGateResult {
            decision: GateDecision::Continue,
            auto_fix_suggestions: vec![],
            message: "节奏审查无建议".into(),
        };
    }
    if !auto.is_empty() && continuous {
        let n = auto.len();
        return PacingGateResult {
            decision: GateDecision::AutoFix,
            auto_fix_suggestions: auto,
            // Pacing never blocks publish; Autofix is advisory local polish only.
            message: format!("节奏建议 {n} 条将局部修订（不阻断发布）"),
        };
    }
    let n = auto.len() + rest.len();
    PacingGateResult {
        decision: GateDecision::AwaitHuman,
        auto_fix_suggestions: auto,
        message: format!("节奏审查待确认（{n} 条；节奏问题不阻断发布）"),
    }
}

/// Progress-line / pipeline hints for human choice — per P0, not binary revise/accept.
pub fn consistency_human_option_labels(issues: &[Value]) -> Vec<String> {
    let issues = with_issue_ids(issues.to_vec());
    let p0: Vec<&Value> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .collect();
    let mut opts = Vec::new();
    for issue in p0.iter().take(6) {
        let ty = issue_type(issue);
        let msg = issue
            .get("message")
            .or_else(|| issue.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("问题");
        let short: String = msg.chars().take(22).collect();
        if ty.is_empty() {
            opts.push(format!("修：{short}"));
        } else {
            opts.push(format!("修：{ty} {short}"));
        }
    }
    if p0.len() > 1 {
        opts.push("修全部阻断项".into());
    } else if opts.is_empty() && !issues.is_empty() {
        opts.push("按清单局部修订".into());
    }
    opts.push("接受并结束本轮".into());
    opts
}

pub fn should_publish(consistency_passed: bool, has_blocking: bool) -> bool {
    consistency_passed && !has_blocking
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn consistency_autofix_mentions_full_revise_for_timeline_p0() {
        let issues = vec![json!({
            "type": "TIMELINE",
            "priority": "P0",
            "message": "倒计时回跳，时间互斥",
        })];
        let gate = on_consistency_result(false, &issues, true);
        assert_eq!(gate.decision, GateDecision::AutoFix);
        assert!(gate.message.contains("整章修订"), "{}", gate.message);
    }

    #[test]
    fn passed_with_p2_only_continues_not_await_human() {
        let issues = vec![json!({
            "type": "TIMELINE",
            "priority": "P2",
            "message": "时间流速略显压缩",
        })];
        let gate = on_consistency_result(true, &issues, true);
        assert_eq!(gate.decision, GateDecision::Continue);
        assert!(gate.message.contains("非阻断") || gate.message.contains("通过"));
    }

    #[test]
    fn human_options_are_per_issue_not_binary() {
        let issues = vec![
            json!({"type":"META","priority":"P0","message":"章号元叙述"}),
            json!({"type":"TIMELINE","priority":"P0","message":"倒计时互斥"}),
        ];
        let opts = consistency_human_option_labels(&issues);
        assert!(opts.iter().any(|o| o.contains("META")));
        assert!(opts.iter().any(|o| o.contains("修全部阻断项")));
        assert!(opts.iter().any(|o| o.contains("接受并结束")));
        assert!(!opts.iter().any(|o| o == "按审校局部修订"));
        assert!(!opts.iter().any(|o| o == "接受问题"));
    }

    #[test]
    fn legacy_block_wire_value_deserializes_as_await_human() {
        let d: GateDecision = serde_json::from_str("\"block\"").unwrap();
        assert_eq!(d, GateDecision::AwaitHuman);
        let d2: GateDecision = serde_json::from_str("\"await_human\"").unwrap();
        assert_eq!(d2, GateDecision::AwaitHuman);
    }
}
