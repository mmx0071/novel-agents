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

/// Explicit clocks/countdowns, injury sites, ability loci — candidates for P0.
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
        || t.contains("时间互斥")
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

pub fn issue_type(issue: &Value) -> String {
    issue
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase()
}

fn issue_message(issue: &Value) -> String {
    [
        issue.get("message").and_then(|v| v.as_str()).unwrap_or(""),
        issue
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    ]
    .join(" ")
}

/// Model sometimes emits "checked, no violation" as a fake P0 — drop those.
pub fn is_non_issue(issue: &Value) -> bool {
    let msg = issue_message(issue);
    if msg.trim().is_empty() {
        return true;
    }
    let soft_clear = [
        "无违规",
        "未检出",
        "暂无明文违规",
        "无实质",
        "无实质性",
        "衔接流畅",
        "无矛盾",
        "不构成强制",
        "不阻断",
        "可视为合理",
        "检查发现无",
    ];
    if soft_clear.iter().any(|s| msg.contains(s)) {
        // Keep real contradictions that also mention a soft phrase elsewhere.
        let hard = [
            "互斥",
            "回跳",
            "矛盾",
            "冲突",
            "不一致",
            "破设定",
            "死人",
            "章号元叙述",
        ];
        let has_hard = hard.iter().any(|h| msg.contains(h))
            && !msg.contains("无矛盾")
            && !msg.contains("无违规")
            && !msg.contains("未检出");
        // "时间互斥…；另：无章号违规" — rare; prefer drop only when soft dominates.
        if msg.contains("无违规") || msg.contains("未检出") || msg.contains("暂无明文违规") {
            return true;
        }
        if msg.contains("无实质") || msg.contains("衔接流畅") {
            return !has_hard;
        }
        return !has_hard;
    }
    false
}

fn has_hard_conflict_markers(msg: &str) -> bool {
    [
        "互斥",
        "回跳",
        "矛盾",
        "冲突",
        "不一致",
        "破设定",
        "不可加总",
        "前后不符",
        "无视钩子",
        "死人",
    ]
    .iter()
    .any(|h| msg.contains(h))
}

fn is_outline_key_events_missing(msg: &str) -> bool {
    msg.contains("关键事件完全缺失")
        || (msg.contains("关键事件") && msg.contains("完全缺失"))
}

/// Soft denial of hook break — must win over bare 「断档」/「未接」substring matches.
fn has_hook_soft_denial(msg: &str) -> bool {
    [
        "不算断档",
        "未造成断档",
        "不构成断档",
        "没有断档",
        "无断档",
        "未断档",
        "信息上已承接",
        "已承接",
        "虽接上",
        "略有跳接",
        "节奏上略有",
        "标为P1",
        "因此标为P1",
        "而非P0",
        "不算P0",
    ]
    .iter()
    .any(|s| msg.contains(s))
}

/// Opening failed to pick up prior-chapter hook — publish blocker language.
fn is_hook_continuity_blocker(msg: &str) -> bool {
    // Auditor often writes "不算断档 / 虽接上…略有跳接" while still tagging P0.
    if has_hook_soft_denial(msg) {
        return false;
    }
    msg.contains("无视钩子")
        || msg.contains("未承接")
        || msg.contains("未接住")
        || msg.contains("钩子蒸发")
        || msg.contains("跳过钩子")
        // Bare「断档」only when not denied above.
        || msg.contains("断档")
        || (msg.contains("开篇")
            && (msg.contains("未承接")
                || msg.contains("未接住")
                || msg.contains("完全另起")
                || msg.contains("另起无关")
                || msg.contains("另起场景")))
}

/// Soft hedging that means "not a publish blocker" even when typed as TIMELINE/ABILITY_LOC.
fn has_soft_nonblocker_markers(msg: &str) -> bool {
    [
        "不算硬冲突",
        "不构成硬",
        "不构成强制",
        "可接受",
        "微小波动",
        "略有压缩",
        "时间压缩感",
        "略模糊",
        "建议",
        "可能让读者",
        "读感",
        "防止设定质疑",
        "可补",
        "不阻断",
        "视觉错觉",
        "没有回跳",
        "无回跳",
        "数值连续",
        "无停滞",
        "不算断档",
        "未造成断档",
        "不构成断档",
        "虽接上",
        "略有跳接",
        "标为P1",
        "而非P0",
    ]
    .iter()
    .any(|s| msg.contains(s))
}

/// Soft narrative gaps must not block publish as P0.
fn should_demote_to_p1(issue: &Value) -> bool {
    let ty = issue_type(issue);
    let msg = issue_message(issue);
    // Explicit soft verdict wins even if type is TIMELINE/ABILITY_LOC and model said P0.
    if has_soft_nonblocker_markers(&msg) && !msg.contains("回跳到") && !msg.contains("倒计时回跳")
    {
        // "没有回跳…不算硬冲突" → demote; "从14:52回跳到14:58" → keep hard.
        if !(msg.contains("回跳") && !msg.contains("没有回跳") && !msg.contains("无回跳")) {
            return true;
        }
    }
    match ty.as_str() {
        // Plot-card exit is owned by plot_acceptor; incomplete progress ≠ publish blocker.
        "PLOT" => true,
        // Soft outline gaps demote; total missing key events stay hard (see escalate).
        "OUTLINE" => !is_outline_key_events_missing(&msg),
        "TIMELINE" => {
            // Compression / pacing feel without hard jump → P1.
            (msg.contains("略有")
                || msg.contains("偏少")
                || msg.contains("压缩")
                || msg.contains("读感")
                || msg.contains("跳动次数"))
                && !msg.contains("回跳到")
                && !msg.contains("互斥")
        }
        "ABILITY_LOC" => {
            // Suggestion to clarify illusion / prevent misread ≠ locus contradiction.
            msg.contains("建议")
                || msg.contains("略模糊")
                || msg.contains("可能让读者")
                || msg.contains("视觉错觉")
                || msg.contains("防止设定质疑")
                || (!has_hard_conflict_markers(&msg)
                    && (msg.contains("误以为") || msg.contains("易误导")))
        }
        "CONTINUITY" => {
            has_hook_soft_denial(&msg)
                || (!is_hook_continuity_blocker(&msg)
                    && (msg.contains("略有")
                        || msg.contains("可补")
                        || msg.contains("铺垫")
                        || msg.contains("跳跃")
                        || msg.contains("跳接")
                        || msg.contains("偏长")))
        }
        "LORE" | "CHARACTER" | "POV" => {
            !msg.contains("破设定")
                && !msg.contains("死人")
                && (msg.contains("可补")
                    || msg.contains("缺乏")
                    || msg.contains("动机")
                    || msg.contains("铺垫")
                    || msg.contains("偏松")
                    || msg.contains("P1")
                    || msg.contains("次要"))
        }
        "META" => msg.contains("虽未直接") || msg.contains("潜在检查") || msg.contains("需确认"),
        _ => false,
    }
}

fn should_escalate_to_p0(issue: &Value) -> bool {
    if is_non_issue(issue) || should_demote_to_p1(issue) {
        return false;
    }
    let ty = issue_type(issue);
    let msg = issue_message(issue);
    match ty.as_str() {
        // Only hard factual contradictions escalate — not "type=TIMELINE" alone.
        "TIMELINE" | "INJURY" | "ABILITY_LOC" => has_hard_conflict_markers(&msg),
        // Do NOT scan quote — clocks in evidence were falsely escalating PLOT/LORE to P0.
        "OUTLINE" => is_outline_key_events_missing(&msg),
        "META" => {
            msg.contains("第")
                && msg.contains("章")
                && (msg.contains("出现") || msg.contains("指称") || msg.contains("元叙述"))
                && !msg.contains("未检出")
                && !msg.contains("无违规")
        }
        "CONTINUITY" => is_hook_continuity_blocker(&msg),
        "" => {
            is_factual_anchor_issue(&msg)
                && (msg.contains("互斥")
                    || msg.contains("矛盾")
                    || msg.contains("冲突")
                    || msg.contains("不一致")
                    || is_hook_continuity_blocker(&msg))
        }
        _ => false,
    }
}

/// Normalize priorities: drop non-issues, demote soft P0, escalate real factual anchors.
/// Returns `(has_p0, normalized_issues)`.
pub fn normalize_consistency_issues(issues: Vec<Value>) -> (bool, Vec<Value>) {
    let mut has_p0 = false;
    let out: Vec<Value> = issues
        .into_iter()
        .filter(|issue| !is_non_issue(issue))
        .map(|mut issue| {
            let mut pri = issue_priority(&issue);
            if should_escalate_to_p0(&issue) {
                pri = "P0".into();
            }
            // Demote after escalate — soft wording must win over type-based P0.
            if pri == "P0" && should_demote_to_p1(&issue) {
                pri = "P1".into();
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
    let out = with_issue_ids(out);
    let has_p0 = has_p0 || out.iter().any(|i| issue_priority(i) == "P0");
    (has_p0, out)
}

/// Assign stable `id` fields (`p0-meta-1`) for per-issue revise gates.
pub fn with_issue_ids(issues: Vec<Value>) -> Vec<Value> {
    issues
        .into_iter()
        .enumerate()
        .map(|(i, mut issue)| {
            let existing = issue
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let id = existing.unwrap_or_else(|| {
                let pri = issue_priority(&issue).to_ascii_lowercase();
                let ty = {
                    let t = issue_type(&issue).to_ascii_lowercase();
                    if t.is_empty() {
                        "issue".into()
                    } else {
                        t.replace('_', "-")
                    }
                };
                format!("{pri}-{ty}-{}", i + 1)
            });
            if let Some(obj) = issue.as_object_mut() {
                obj.insert("id".into(), json!(id));
            }
            issue
        })
        .collect()
}

/// Keep only issues whose `id` is in `ids` (order preserved). Empty `ids` → no filter.
pub fn filter_issues_by_ids(issues: &[Value], ids: &[String]) -> Vec<Value> {
    if ids.is_empty() {
        return issues.to_vec();
    }
    let set: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    issues
        .iter()
        .filter(|i| {
            i.get("id")
                .and_then(|v| v.as_str())
                .map(|id| set.contains(id))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
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

/// True when audit still has a blocking TIMELINE P0 (local patch usually cannot fix).
pub fn has_timeline_p0(issues: &[Value]) -> bool {
    issues.iter().any(|i| {
        issue_type(i) == "TIMELINE" && issue_priority(i) == "P0" && !is_non_issue(i)
    })
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
            "type": "TIMELINE",
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

    #[test]
    fn drops_meta_no_violation() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "META",
            "priority": "P0",
            "message": "检查发现无章号元叙述违规，暂无明文违规",
            "quote": "你好，我是甲。",
        })]);
        assert!(!has_p0);
        assert!(issues.is_empty());
    }

    #[test]
    fn demotes_soft_plot_and_lore() {
        let (has_p0, issues) = normalize_consistency_issues(vec![
            json!({
                "type": "PLOT",
                "priority": "P0",
                "message": "剧情卡收束条件未全部兑现，关键落点未抵达",
                "quote": "午后三刻",
            }),
            json!({
                "type": "LORE",
                "priority": "P0",
                "message": "主角态度转变缺乏动机铺垫，可补交代",
            }),
            json!({
                "type": "CONTINUITY",
                "priority": "P0",
                "message": "钩子承接略有跳跃，整体衔接流畅，无实质性断裂",
            }),
        ]);
        assert!(!has_p0, "{issues:?}");
        assert_eq!(issues.len(), 2, "continuity non-issue dropped; plot/lore demoted");
        assert!(issues.iter().all(|i| i["priority"] == "P1"));
    }

    #[test]
    fn quote_clock_does_not_escalate_plot() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "PLOT",
            "priority": "P1",
            "message": "剧情卡关键落点未达成",
            "quote": "日晷仍停在午后三刻",
        })]);
        assert!(!has_p0);
        assert_eq!(issues[0]["priority"], "P1");
    }

    #[test]
    fn timeline_p0_detected() {
        let issues = vec![json!({
            "type": "TIMELINE",
            "priority": "P0",
            "message": "倒计时从14:52回跳到14:58，时间互斥",
        })];
        assert!(has_timeline_p0(&issues));
    }

    #[test]
    fn demotes_soft_timeline_that_denies_hard_conflict() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "TIMELINE",
            "priority": "P0",
            "message": "末段倒计时跳动次数偏少，略有压缩感；数值序列没有回跳或停滞，属于可接受的微小波动，不算硬冲突。",
        })]);
        assert!(!has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P1");
    }

    #[test]
    fn demotes_soft_ability_loc_suggestion() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "ABILITY_LOC",
            "priority": "P0",
            "message": "掌心发光易被误读为锚点转移；建议强化‘视觉错觉’解释，当前写法略模糊，可能让读者误以为位置变化。",
        })]);
        assert!(!has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P1");
    }

    #[test]
    fn hard_timeline_jump_still_p0() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "TIMELINE",
            "priority": "P1",
            "message": "倒计时从14:52回跳到14:58，时间互斥",
        })]);
        assert!(has_p0);
        assert_eq!(issues[0]["priority"], "P0");
        assert!(has_timeline_p0(&issues));
    }

    #[test]
    fn outline_key_events_missing_stays_p0() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "OUTLINE",
            "priority": "P1",
            "message": "章纲关键事件完全缺失：正文未出现对峙与夺钥",
        })]);
        assert!(has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P0");
    }

    #[test]
    fn soft_outline_still_demoted() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "OUTLINE",
            "priority": "P0",
            "message": "章纲符合度略低，次要场面可补",
        })]);
        assert!(!has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P1");
    }

    #[test]
    fn hook_not_picked_up_is_p0() {
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "CONTINUITY",
            "priority": "P1",
            "message": "开篇另起场景，未承接前章章末钩子，形成断档",
        })]);
        assert!(has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P0");
    }

    #[test]
    fn soft_hook_not_break_denial_demotes() {
        // Real ch16 false P0: model says 不算断档 / 略有跳接 but tags P0.
        let (has_p0, issues) = normalize_consistency_issues(vec![json!({
            "type": "CONTINUITY",
            "priority": "P0",
            "message": "开篇从落地开始，虽接上了落地动作，节奏上略有跳接感，信息上已承接，不算断档。",
        })]);
        assert!(!has_p0, "{issues:?}");
        assert_eq!(issues[0]["priority"], "P1");
    }
}
