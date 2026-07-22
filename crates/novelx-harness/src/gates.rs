use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::review_priority::partition_issues;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    Continue,
    AwaitHuman,
    AutoFix,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyGateResult {
    pub decision: GateDecision,
    pub auto_fix_issues: Vec<Value>,
    pub message: String,
}

pub fn on_consistency_result(passed: bool, issues: &[Value], continuous: bool) -> ConsistencyGateResult {
    let (auto, rest) = partition_issues(issues);
    if passed && rest.is_empty() && auto.is_empty() {
        return ConsistencyGateResult {
            decision: GateDecision::Continue,
            auto_fix_issues: vec![],
            message: "一致性审计通过".into(),
        };
    }
    if !auto.is_empty() && continuous {
        let n = auto.len();
        return ConsistencyGateResult {
            decision: GateDecision::AutoFix,
            auto_fix_issues: auto,
            message: format!("发现 {n} 条 P0/P1 问题，将局部自动修复"),
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
            message: format!("节奏建议 {n} 条将局部修订"),
        };
    }
    let n = auto.len() + rest.len();
    PacingGateResult {
        decision: GateDecision::AwaitHuman,
        auto_fix_suggestions: auto,
        message: format!("节奏审查待确认（{n} 条）"),
    }
}

pub fn should_publish(consistency_passed: bool, has_blocking: bool) -> bool {
    consistency_passed && !has_blocking
}
