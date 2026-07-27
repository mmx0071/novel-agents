//! Pre-apply audit helpers for mutating tools.

use crate::ToolResult;
use novelx_pipeline::SettingAuditResult;
use serde_json::{json, Value};

/// When audit found BLOCKER and force is off, return a blocked ToolResult (no preview/apply).
/// `soft_escape`: setup / first-write path — still attach audit in preview, but do not hard-block.
pub fn require_audit_pass(audit: &SettingAuditResult, force: bool) -> Option<ToolResult> {
    require_audit_pass_with(audit, force, false)
}

pub fn require_audit_pass_with(
    audit: &SettingAuditResult,
    force: bool,
    soft_escape: bool,
) -> Option<ToolResult> {
    if audit.blocker && !force && !soft_escape {
        return Some(ToolResult {
            output: format!(
                "设定/结构审计 BLOCKER，未进入可应用预览：{}\n{}",
                audit.summary, audit.report
            ),
            data: json!({
                "blocked": true,
                "reason": "audit_blocker",
                "audit": audit.raw,
            }),
        });
    }
    None
}

/// Compact audit object for mutation preview cards.
pub fn audit_preview_value(audit: &SettingAuditResult) -> Value {
    json!({
        "blocker": audit.blocker,
        "skipped": audit.skipped,
        "summary": audit.summary,
        "report": audit.report.chars().take(800).collect::<String>(),
        "raw": audit.raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use novelx_pipeline::SettingAuditResult;

    #[test]
    fn blocker_hard_stops_without_force() {
        let audit = SettingAuditResult {
            blocker: true,
            skipped: false,
            summary: "冲突".into(),
            report: "detail".into(),
            raw: json!({"passed": false}),
        };
        assert!(require_audit_pass(&audit, false).is_some());
        assert!(require_audit_pass(&audit, true).is_none());
    }

    #[test]
    fn warning_or_skip_allows_preview() {
        let audit = SettingAuditResult {
            blocker: false,
            skipped: true,
            summary: "审计跳过".into(),
            report: String::new(),
            raw: json!({}),
        };
        assert!(require_audit_pass(&audit, false).is_none());
    }

    #[test]
    fn soft_escape_allows_blocker_during_setup() {
        let audit = SettingAuditResult {
            blocker: true,
            skipped: false,
            summary: "冲突".into(),
            report: "detail".into(),
            raw: json!({"passed": false}),
        };
        assert!(require_audit_pass_with(&audit, false, true).is_none());
        assert!(require_audit_pass_with(&audit, false, false).is_some());
    }
}
