//! Per-issue audit decision card (no product-specific novel names).

use novelx_core::audit_decisions::{
    build_fallback_audit_decisions, format_audit_issue_checklist, resolve_decision_pick,
    validate_offered_decisions,
};
use serde_json::json;

#[test]
fn compact_checklist_not_full_report() {
    let issues = vec![
        json!({
            "type": "META",
            "priority": "P0",
            "message": "叙述中出现章号指称",
            "location": "第4段",
            "quote": "他走了五章的路",
        }),
        json!({
            "type": "TIMELINE",
            "priority": "P0",
            "message": "明确时间互斥",
            "location": "第1段",
        }),
        json!({
            "type": "CONTINUITY",
            "priority": "P1",
            "message": "钩子承接略有跳跃",
        }),
    ];
    let brief = format_audit_issue_checklist(&issues, 17);
    assert!(brief.contains("问题清单"));
    assert!(brief.contains("阻断 **2**"));
    assert!(brief.contains("META"));
    assert!(!brief.contains("【已核对】"));
    assert!(!brief.contains("一致性审计报告"));
}

#[test]
fn fallback_options_target_single_issue() {
    let issues = vec![
        json!({"id":"p0-meta-1","type":"META","priority":"P0","message":"章号元叙述"}),
        json!({"id":"p0-timeline-2","type":"TIMELINE","priority":"P0","message":"倒计时互斥"}),
        json!({"id":"p0-outline-3","type":"OUTLINE","priority":"P0","message":"关键事件缺失"}),
    ];
    let opts = build_fallback_audit_decisions("sample-novel", 17, &issues, false);
    assert!(opts.iter().any(|o| o.id == "fix_p0-meta-1"));
    assert!(opts.iter().any(|o| o.id == "fix_p0-timeline-2"));
    assert!(opts.iter().any(|o| o.id == "fix_all_p0"));
    assert!(opts.iter().any(|o| o.id == "accept"));
    assert!(!opts.iter().any(|o| o.label == "按审校局部修订"));

    let (tool, args) = resolve_decision_pick("修：META 章号元叙述", &opts)
        .or_else(|| resolve_decision_pick("1", &opts))
        .expect("pick");
    assert_eq!(tool, "steer_run");
    assert_eq!(args["choice"], "revise");
    let ids = args["issue_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn offer_decisions_rejects_unknown_issue_id() {
    let issues = vec![json!({"id":"p0-meta-1","type":"META","priority":"P0","message":"x"})];
    let err = validate_offered_decisions(
        "sample-novel",
        17,
        &issues,
        &[json!({
            "id": "bad",
            "label": "修幽灵问题",
            "action": "revise",
            "issue_ids": ["no-such-id"],
        })],
        false,
    )
    .unwrap_err();
    assert!(err.contains("未知 issue_id"));
}
