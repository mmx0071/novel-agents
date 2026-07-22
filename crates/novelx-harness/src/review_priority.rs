use serde_json::{json, Value};
use std::collections::HashSet;

pub const AUTO_FIX_PRIORITIES: &[&str] = &["P0", "P1"];

pub fn normalize_priority(raw: &str) -> String {
    let u = raw.trim().to_uppercase();
    if u.starts_with("P0")
        || u == "BLOCKER"
        || u.contains("CRITICAL")
        || u.contains("阻断")
    {
        "P0".into()
    } else if u.starts_with("P1")
        || u == "WARN"
        || u == "WARNING"
        || u.contains("HIGH")
        || u.contains("重要")
    {
        "P1".into()
    } else {
        "P2".into()
    }
}

/// Read `priority` or legacy `severity`, then normalize.
pub fn issue_priority(issue: &Value) -> String {
    let raw = issue
        .get("priority")
        .or_else(|| issue.get("severity"))
        .and_then(|v| v.as_str())
        .unwrap_or("P1");
    normalize_priority(raw)
}

/// Explicit clocks/countdowns, injury sites, ability loci — always P0 when flagged.
pub fn is_factual_anchor_issue(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let upper = t.to_uppercase();
    if upper.contains("TIMELINE")
        || upper.contains("INJURY")
        || upper.contains("ABILITY_LOC")
        || upper.contains("ABILITY-LOC")
    {
        return true;
    }

    let time_anchor = t.contains("倒计时")
        || t.contains("时间线")
        || t.contains("时间信息")
        || has_clock_token(t)
        || (t.contains("小时")
            && (t.contains("分钟")
                || t.contains("点")
                || t.contains("分")
                || has_clock_token(t)
                || t.contains("之后")
                || t.contains("经过")
                || t.contains("过了")));

    let injury_anchor = (t.contains("伤") || t.contains("伤口") || t.contains("受伤") || t.contains("伤势"))
        && (t.contains("部位")
            || t.contains("左手")
            || t.contains("右手")
            || t.contains("左臂")
            || t.contains("右臂")
            || t.contains("左腿")
            || t.contains("右腿")
            || t.contains("肩膀")
            || t.contains("肩胛")
            || t.contains("腹部")
            || t.contains("胸口")
            || t.contains("胸膛")
            || t.contains("背部")
            || t.contains("额头")
            || t.contains("腕")
            || t.contains("踝")
            || t.contains("肋骨"));

    let ability_loc_anchor = (t.contains("能力") || t.contains("权能") || t.contains("异能") || t.contains("力量"))
        && (t.contains("位置")
            || t.contains("所在")
            || t.contains("寄居")
            || t.contains("附着")
            || t.contains("藏于")
            || t.contains("位于")
            || t.contains("载体")
            || t.contains("寄宿"));

    time_anchor || injury_anchor || ability_loc_anchor
}

fn has_clock_token(t: &str) -> bool {
    // 5:42 / 5：42 / 17:30
    if t.contains(':') || t.contains('：') {
        return true;
    }
    // 「凌晨四点」「早上七点十五分」等也算明确时间锚
    let clock_words = [
        "凌晨", "清晨", "早上", "上午", "中午", "下午", "傍晚", "晚上", "夜里", "午夜",
    ];
    clock_words.iter().any(|w| t.contains(w))
        && (t.contains("点") || t.contains("分") || t.chars().any(|c| c.is_ascii_digit()))
}

fn issue_text_blob(issue: &Value) -> String {
    let parts = [
        issue.get("type").and_then(|v| v.as_str()).unwrap_or(""),
        issue.get("message").and_then(|v| v.as_str()).unwrap_or(""),
        issue
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        issue.get("quote").and_then(|v| v.as_str()).unwrap_or(""),
        issue
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    ];
    parts.join(" ")
}

/// Normalize priorities; escalate factual-anchor contradictions to P0.
/// Returns `(has_p0, normalized_issues)`.
pub fn normalize_consistency_issues(issues: Vec<Value>) -> (bool, Vec<Value>) {
    let mut has_p0 = false;
    let out: Vec<Value> = issues
        .into_iter()
        .map(|mut issue| {
            let blob = issue_text_blob(&issue);
            let mut pri = issue_priority(&issue);
            if is_factual_anchor_issue(&blob) {
                pri = "P0".into();
            }
            if let Some(obj) = issue.as_object_mut() {
                obj.insert("priority".into(), json!(pri.clone()));
            }
            if pri == "P0" {
                has_p0 = true;
            }
            issue
        })
        .collect();
    (has_p0, out)
}

pub fn partition_issues(issues: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let auto: HashSet<&str> = AUTO_FIX_PRIORITIES.iter().copied().collect();
    let mut p01 = Vec::new();
    let mut p2 = Vec::new();
    for issue in issues {
        let pri = issue_priority(issue);
        if auto.contains(pri.as_str()) {
            p01.push(issue.clone());
        } else {
            p2.push(issue.clone());
        }
    }
    (p01, p2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocker_maps_to_p0() {
        assert_eq!(normalize_priority("BLOCKER"), "P0");
        assert_eq!(normalize_priority("WARN"), "P1");
    }

    #[test]
    fn clock_countdown_mismatch_is_factual_p0() {
        let msg = "时间信息前后不一致（5:42→6小时 vs 倒计时11.5小时）";
        assert!(is_factual_anchor_issue(msg));
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "priority": "P1",
            "message": msg,
        })]);
        assert!(has_p0);
        assert_eq!(issues[0]["priority"], "P0");
    }

    #[test]
    fn injury_site_is_factual_p0() {
        assert!(is_factual_anchor_issue(
            "受伤部位矛盾：前章左肩中弹，本章写成右臂包扎"
        ));
    }

    #[test]
    fn ability_locus_is_factual_p0() {
        assert!(is_factual_anchor_issue(
            "能力所在位置冲突：此前寄宿在断剑，本章写成附着左腕"
        ));
    }

    #[test]
    fn style_nit_not_escalated() {
        assert!(!is_factual_anchor_issue("用词略重复，可读性一般"));
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "priority": "P2",
            "message": "用词略重复，可读性一般",
        })]);
        assert!(!has_p0);
        assert_eq!(issues[0]["priority"], "P2");
    }
}
