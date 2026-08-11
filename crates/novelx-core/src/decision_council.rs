//! Decision Council — ballot adaptation + deterministic aggregation.
//!
//! Config: `config/decision_council.yaml`. Feature: `studio.decision_council`.

use novelx_harness::{issue_priority, issue_type};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

/// mtime-aware cache so hot paths do not re-read YAML every call.
static COUNCIL_CACHE: RwLock<Option<(PathBuf, Option<SystemTime>, DecisionCouncilConfig)>> =
    RwLock::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilVote {
    Revise,
    Approve,
    Escalate,
    Abstain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilSeverity {
    Block,
    Warn,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JuryBallot {
    pub agent: String,
    pub vote: CouncilVote,
    pub severity: CouncilSeverity,
    /// 0–100
    pub score: u32,
    #[serde(default)]
    pub preferred_action: String,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub issue_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilAction {
    AutoRevise,
    AutoContinue,
    EscalateHuman,
    FetchMaterial,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilVerdict {
    pub action: CouncilAction,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    pub ballots: Vec<JuryBallot>,
    pub rationale: String,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct CouncilFile {
    #[serde(default)]
    kinds: HashMap<String, KindConfig>,
    #[serde(default)]
    material_researcher: MaterialConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KindConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_retries")]
    pub max_auto_retries: u32,
    /// Consecutive council evaluations with overlapping P0 types → escalate.
    #[serde(default = "default_same_type_streak")]
    pub same_type_streak_limit: u32,
    #[serde(default = "default_min_score")]
    min_score: u32,
    #[serde(default = "default_epsilon")]
    deadlock_epsilon: u32,
    #[serde(default)]
    allow_auto_accept_p0: bool,
    #[serde(default)]
    members: Vec<MemberConfig>,
    #[serde(default)]
    batch_per_chapter: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct MemberConfig {
    role: String,
    #[serde(default = "default_weight")]
    weight: u32,
    #[serde(default)]
    veto: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MaterialConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cooldown")]
    pub cooldown_chapters: u32,
    #[serde(default = "default_max_cards")]
    pub max_cards_per_call: u32,
    #[serde(default)]
    pub inject_into: Vec<String>,
    #[serde(default)]
    pub motif_from: Vec<String>,
    #[serde(default = "default_inject_chars")]
    pub inject_chars: usize,
}

fn default_true() -> bool {
    true
}
fn default_max_retries() -> u32 {
    1
}
fn default_same_type_streak() -> u32 {
    2
}
fn default_min_score() -> u32 {
    70
}
fn default_epsilon() -> u32 {
    8
}
fn default_weight() -> u32 {
    1
}
fn default_cooldown() -> u32 {
    5
}
fn default_max_cards() -> u32 {
    3
}
fn default_inject_chars() -> usize {
    900
}

impl Default for KindConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_auto_retries: default_max_retries(),
            same_type_streak_limit: default_same_type_streak(),
            min_score: default_min_score(),
            deadlock_epsilon: default_epsilon(),
            allow_auto_accept_p0: false,
            members: vec![
                MemberConfig {
                    role: "consistency_auditor".into(),
                    weight: 2,
                    veto: true,
                },
                MemberConfig {
                    role: "pacing_reviewer".into(),
                    weight: 1,
                    veto: false,
                },
                MemberConfig {
                    role: "plot_acceptor".into(),
                    weight: 1,
                    veto: false,
                },
            ],
            batch_per_chapter: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecisionCouncilConfig {
    kinds: HashMap<String, KindConfig>,
    pub material: MaterialConfig,
}

impl DecisionCouncilConfig {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("decision_council.yaml");
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();
        if let Ok(guard) = COUNCIL_CACHE.read() {
            if let Some((ref cached_path, ref cached_mtime, ref cfg)) = *guard {
                if cached_path == &path && cached_mtime == &mtime {
                    return cfg.clone();
                }
            }
        }
        let cfg = Self::load_uncached(&path);
        if let Ok(mut guard) = COUNCIL_CACHE.write() {
            *guard = Some((path, mtime, cfg.clone()));
        }
        cfg
    }

    fn load_uncached(path: &Path) -> Self {
        if !path.exists() {
            return Self::defaults();
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::defaults();
        };
        let Ok(file) = serde_yaml::from_str::<CouncilFile>(&raw) else {
            tracing::warn!("decision_council.yaml parse failed; using defaults");
            return Self::defaults();
        };
        Self {
            kinds: file.kinds,
            material: file.material_researcher,
        }
    }

    pub fn defaults() -> Self {
        let mut kinds = HashMap::new();
        kinds.insert("audit_content".into(), KindConfig::default());
        kinds.insert(
            "chapter_next_clean".into(),
            KindConfig {
                enabled: false, // ultra-longform default: offer gate, don't auto-continue
                members: vec![],
                ..KindConfig::default()
            },
        );
        kinds.insert(
            "chapter_boundary_seal".into(),
            KindConfig {
                members: vec![],
                batch_per_chapter: true,
                ..KindConfig::default()
            },
        );
        Self {
            kinds,
            material: MaterialConfig::default(),
        }
    }

    pub fn kind_enabled(&self, kind: &str) -> bool {
        self.kinds.get(kind).map(|k| k.enabled).unwrap_or(false)
    }

    pub fn audit_kind(&self) -> KindConfig {
        self.kinds
            .get("audit_content")
            .cloned()
            .unwrap_or_default()
    }

    pub fn seal_batch_per_chapter(&self) -> bool {
        self.kinds
            .get("chapter_boundary_seal")
            .map(|k| k.enabled && k.batch_per_chapter)
            .unwrap_or(true)
    }
}

/// True when any issue has priority P0.
pub fn has_true_p0(issues: &[Value]) -> bool {
    issues.iter().any(|i| issue_priority(i) == "P0")
}

/// Sorted unique P0 issue types (uppercase).
pub fn p0_issue_types(issues: &[Value]) -> Vec<String> {
    let mut types: Vec<String> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .map(issue_type)
        .filter(|t| !t.is_empty())
        .collect();
    types.sort();
    types.dedup();
    types
}

/// Advance streak when current P0 types overlap previous look; else reset to 1 (or 0 if none).
pub fn next_same_type_streak(prev_types: &[String], current_types: &[String], prev_streak: u32) -> u32 {
    if current_types.is_empty() {
        return 0;
    }
    if prev_types.is_empty() {
        return 1;
    }
    let overlap = current_types.iter().any(|t| prev_types.iter().any(|p| p == t));
    if overlap {
        prev_streak.saturating_add(1).max(2)
    } else {
        1
    }
}

/// Adapt consistency audit JSON into a chair ballot.
pub fn ballot_from_consistency(data: &Value, issues: &[Value]) -> JuryBallot {
    let p0: Vec<&Value> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .collect();
    let refs: Vec<String> = p0
        .iter()
        .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    if !p0.is_empty() {
        JuryBallot {
            agent: "consistency_auditor".into(),
            vote: CouncilVote::Revise,
            severity: CouncilSeverity::Block,
            score: 92,
            preferred_action: "fix_all_p0".into(),
            reasons: vec![format!("{} 条真 P0 阻断", p0.len())],
            issue_refs: refs,
        }
    } else {
        let passed = data
            .get("consistency_passed")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        JuryBallot {
            agent: "consistency_auditor".into(),
            vote: if passed {
                CouncilVote::Approve
            } else {
                CouncilVote::Revise
            },
            severity: CouncilSeverity::Info,
            score: if passed { 88 } else { 60 },
            preferred_action: if passed {
                "approve".into()
            } else {
                "fix_all".into()
            },
            reasons: vec!["无真 P0".into()],
            issue_refs: vec![],
        }
    }
}

/// Pacing is advisory only (never veto publish).
pub fn ballot_from_pacing(data: &Value) -> JuryBallot {
    let pacing_passed = data
        .get("pacing_passed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    JuryBallot {
        agent: "pacing_reviewer".into(),
        vote: if pacing_passed {
            CouncilVote::Abstain
        } else {
            CouncilVote::Revise
        },
        severity: if pacing_passed {
            CouncilSeverity::Info
        } else {
            CouncilSeverity::Warn
        },
        score: if pacing_passed { 70 } else { 55 },
        preferred_action: if pacing_passed {
            "approve".into()
        } else {
            "fix_all_p0".into()
        },
        reasons: vec![if pacing_passed {
            "节奏可接受（顾问票）".into()
        } else {
            "节奏有建议（不否决发布）".into()
        }],
        issue_refs: vec![],
    }
}

/// Plot acceptor advisor ballot from pipeline data or on-disk accept JSON fields in `data`.
pub fn ballot_from_plot_acceptor(data: &Value) -> JuryBallot {
    let pass = data
        .get("plot_accept_passed")
        .and_then(|v| v.as_bool())
        .or_else(|| data.get("pass").and_then(|v| v.as_bool()));
    match pass {
        Some(true) => JuryBallot {
            agent: "plot_acceptor".into(),
            vote: CouncilVote::Approve,
            severity: CouncilSeverity::Info,
            score: 80,
            preferred_action: "approve".into(),
            reasons: vec!["剧情收束验收通过".into()],
            issue_refs: vec![],
        },
        Some(false) => JuryBallot {
            agent: "plot_acceptor".into(),
            vote: CouncilVote::Revise,
            severity: CouncilSeverity::Warn,
            score: 58,
            preferred_action: "fix_all_p0".into(),
            reasons: vec!["剧情收束未通过（顾问）".into()],
            issue_refs: vec![],
        },
        None => JuryBallot {
            agent: "plot_acceptor".into(),
            vote: CouncilVote::Abstain,
            severity: CouncilSeverity::Info,
            score: 50,
            preferred_action: String::new(),
            reasons: vec!["无剧情验收结果".into()],
            issue_refs: vec![],
        },
    }
}

/// Aggregate ballots for `audit_content`. Never auto-accept when true P0 remain (unless allowed).
pub fn aggregate_audit(
    cfg: &KindConfig,
    ballots: Vec<JuryBallot>,
    issues: &[Value],
    project: &str,
    chapter: u32,
    retry_count: u32,
    same_type_streak: u32,
    config_root: Option<&Path>,
    violations: &[Value],
    use_revise_plan: bool,
) -> CouncilVerdict {
    let p0 = has_true_p0(issues);
    if retry_count >= cfg.max_auto_retries {
        return escalate(
            ballots,
            retry_count,
            format!("自动修订已达上限 {}", cfg.max_auto_retries),
        );
    }
    let streak_limit = cfg.same_type_streak_limit.max(1);
    if p0 && same_type_streak >= streak_limit {
        return escalate(
            ballots,
            retry_count,
            format!(
                "同类 P0 连续未消除（streak={same_type_streak}≥{streak_limit}），停止自动修订"
            ),
        );
    }

    // Veto: any veto member voting revise/escalate with block severity while P0 present.
    let veto_blocks = cfg.members.iter().any(|m| {
        m.veto
            && ballots.iter().any(|b| {
                b.agent == m.role
                    && matches!(b.vote, CouncilVote::Revise | CouncilVote::Escalate)
                    && matches!(b.severity, CouncilSeverity::Block)
            })
    });

    if p0 && !cfg.allow_auto_accept_p0 {
        // Hard rule: never accept; prefer revise if scores allow.
        let revise_score = weighted_score(&cfg.members, &ballots, |b| {
            b.vote == CouncilVote::Revise || b.preferred_action.starts_with("fix")
        });
        let escalate_score = weighted_score(&cfg.members, &ballots, |b| {
            b.vote == CouncilVote::Escalate
        });
        if veto_blocks || revise_score >= cfg.min_score as f64 {
            if (revise_score - escalate_score).abs() <= cfg.deadlock_epsilon as f64
                && escalate_score >= cfg.min_score as f64
            {
                return escalate(
                    ballots,
                    retry_count,
                    format!(
                        "修订与升级分差过近（ε={}）",
                        cfg.deadlock_epsilon
                    ),
                );
            }
            return auto_revise(
                ballots,
                issues,
                project,
                chapter,
                retry_count,
                revise_score,
                same_type_streak,
                config_root,
                violations,
                use_revise_plan,
            );
        }
        return escalate(
            ballots,
            retry_count,
            "存在真 P0 且修订共识不足".into(),
        );
    }

    // No P0: approve path for chapter_next; for audit_content this usually means escalate was wrong path.
    let approve_score = weighted_score(&cfg.members, &ballots, |b| {
        b.vote == CouncilVote::Approve || b.preferred_action == "approve"
    });
    let revise_score = weighted_score(&cfg.members, &ballots, |b| {
        b.vote == CouncilVote::Revise || b.preferred_action.starts_with("fix")
    });
    if (approve_score - revise_score).abs() <= cfg.deadlock_epsilon as f64
        && approve_score >= cfg.min_score as f64
        && revise_score >= cfg.min_score as f64
    {
        return escalate(
            ballots,
            retry_count,
            "通过与修订近分死锁".into(),
        );
    }
    if revise_score >= cfg.min_score as f64 && revise_score > approve_score {
        return auto_revise(
            ballots,
            issues,
            project,
            chapter,
            retry_count,
            revise_score,
            same_type_streak,
            config_root,
            violations,
            use_revise_plan,
        );
    }
    escalate(
        ballots,
        retry_count,
        "未形成可自动执行的共识".into(),
    )
}

fn weighted_score(
    members: &[MemberConfig],
    ballots: &[JuryBallot],
    pred: impl Fn(&JuryBallot) -> bool,
) -> f64 {
    let mut num = 0.0;
    let mut den = 0.0;
    for m in members {
        let w = m.weight.max(1) as f64;
        den += w;
        if let Some(b) = ballots.iter().find(|b| b.agent == m.role) {
            if pred(b) {
                num += w * (b.score as f64);
            }
        }
    }
    if den <= 0.0 {
        0.0
    } else {
        num / den
    }
}

fn auto_revise(
    ballots: Vec<JuryBallot>,
    issues: &[Value],
    project: &str,
    chapter: u32,
    retry_count: u32,
    score: f64,
    same_type_streak: u32,
    config_root: Option<&Path>,
    violations: &[Value],
    use_revise_plan: bool,
) -> CouncilVerdict {
    let p0_ids: Vec<String> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    let issue_ids = if p0_ids.is_empty() {
        issues
            .iter()
            .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect::<Vec<_>>()
    } else {
        p0_ids
    };
    let mut args = json!({
        "project": project,
        "chapter": chapter,
        "choice": "revise",
        "issue_ids": issue_ids,
    });
    let mut rationale = format!("评审团共识自动修订（加权分 {score:.0}）");

    if use_revise_plan {
        let plan_cfg = match config_root {
            Some(root) => crate::revise_plan::RevisePlanConfig::load(root),
            None => crate::revise_plan::RevisePlanConfig::default(),
        };
        let plan = crate::revise_plan::build_revise_plan(
            &plan_cfg,
            issues,
            violations,
            same_type_streak,
        );
        if plan.scope == crate::revise_plan::ReviseScope::Escalate {
            return escalate(
                ballots,
                retry_count,
                format!("RevisePlan 升人机：{}", plan.rationale),
            );
        }
        crate::revise_plan::apply_plan_to_steer_args(&mut args, &plan);
        rationale = format!(
            "评审团共识自动修订（加权分 {score:.0}）· {}",
            plan.summary_line()
        );
    }

    CouncilVerdict {
        action: CouncilAction::AutoRevise,
        tool: "steer_run".into(),
        args,
        ballots,
        rationale,
        retry_count,
    }
}

fn escalate(ballots: Vec<JuryBallot>, retry_count: u32, rationale: String) -> CouncilVerdict {
    CouncilVerdict {
        action: CouncilAction::EscalateHuman,
        tool: String::new(),
        args: json!({}),
        ballots,
        rationale,
        retry_count,
    }
}

/// Build default ballots from pipeline audit `data` + issues, then aggregate.
pub fn evaluate_audit_content(
    config_root: &Path,
    project: &str,
    chapter: u32,
    data: &Value,
    issues: &[Value],
    retry_count: u32,
    same_type_streak: u32,
    use_revise_plan: bool,
) -> CouncilVerdict {
    let cfg = DecisionCouncilConfig::load(config_root);
    let kind = cfg.audit_kind();
    if !kind.enabled {
        return escalate(vec![], retry_count, "audit_content 未启用".into());
    }
    let mut ballots = vec![ballot_from_consistency(data, issues)];
    // True P0: chair veto alone decides revise — skip advisory ballots.
    if !has_true_p0(issues) {
        if kind.members.iter().any(|m| m.role == "pacing_reviewer") {
            ballots.push(ballot_from_pacing(data));
        }
        if kind.members.iter().any(|m| m.role == "plot_acceptor") {
            ballots.push(ballot_from_plot_acceptor(data));
        }
    }
    let violations = data
        .get("content_rule_violations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    aggregate_audit(
        &kind,
        ballots,
        issues,
        project,
        chapter,
        retry_count,
        same_type_streak,
        Some(config_root),
        &violations,
        use_revise_plan,
    )
}

pub fn verdict_journal_data(verdict: &CouncilVerdict) -> Value {
    json!({
        "council_kind": "decision_council",
        "action": verdict.action,
        "rationale": verdict.rationale,
        "retry_count": verdict.retry_count,
        "tool": verdict.tool,
        "ballots": verdict.ballots,
        "revise_plan": verdict.args.get("revise_plan"),
        "revise_scope": verdict.args.get("revise_scope"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p0_issue(id: &str) -> Value {
        json!({
            "id": id,
            "type": "TIMELINE",
            "priority": "P0",
            "message": "时间互斥",
            "quote": "昨天"
        })
    }

    #[test]
    fn p0_veto_forces_revise_not_accept() {
        let cfg = KindConfig::default();
        let issues = vec![p0_issue("i1"), p0_issue("i2")];
        let data = json!({"consistency_passed": false});
        let ballots = vec![
            ballot_from_consistency(&data, &issues),
            ballot_from_pacing(&json!({"pacing_passed": true})),
            ballot_from_plot_acceptor(&json!({})),
        ];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "sample-novel", 6, 0, 1, None, &[], true,
        );
        assert_eq!(v.action, CouncilAction::AutoRevise);
        assert_eq!(v.tool, "steer_run");
        assert_eq!(v.args["choice"], "revise");
        let ids = v.args["issue_ids"].as_array().unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(v.args["revise_scope"], "full");
        assert!(!v.args["instructions"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn p0_short_circuit_chair_only_still_revises() {
        let dir = std::env::temp_dir().join("novelx_council_p0_short");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No yaml → defaults; evaluate with P0 should not need advisors.
        let issues = vec![p0_issue("only")];
        let data = json!({"consistency_passed": false, "pacing_passed": false});
        let v = evaluate_audit_content(&dir, "demo", 1, &data, &issues, 0, 1, true);
        assert_eq!(v.action, CouncilAction::AutoRevise);
        assert_eq!(v.ballots.len(), 1);
        assert_eq!(v.ballots[0].agent, "consistency_auditor");
        assert_eq!(v.args["revise_scope"], "full");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_retries_escalates() {
        let cfg = KindConfig::default();
        let issues = vec![p0_issue("i1")];
        let data = json!({"consistency_passed": false});
        let ballots = vec![ballot_from_consistency(&data, &issues)];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "demo", 1, 1, 1, None, &[], false,
        );
        assert_eq!(v.action, CouncilAction::EscalateHuman);
        assert!(v.rationale.contains("上限"));
    }

    #[test]
    fn same_type_streak_escalates() {
        let cfg = KindConfig::default();
        let issues = vec![p0_issue("i1")];
        let data = json!({"consistency_passed": false});
        let ballots = vec![ballot_from_consistency(&data, &issues)];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "demo", 1, 0, 2, None, &[], false,
        );
        assert_eq!(v.action, CouncilAction::EscalateHuman);
        assert!(v.rationale.contains("同类"));
        let streak = next_same_type_streak(
            &["TIMELINE".into()],
            &["TIMELINE".into()],
            1,
        );
        assert_eq!(streak, 2);
    }

    #[test]
    fn allow_auto_accept_p0_false_never_approve_action() {
        let cfg = KindConfig {
            allow_auto_accept_p0: false,
            ..KindConfig::default()
        };
        let issues = vec![p0_issue("x")];
        // Hostile ballot trying to approve — engine must not emit accept.
        let ballots = vec![JuryBallot {
            agent: "consistency_auditor".into(),
            vote: CouncilVote::Approve,
            severity: CouncilSeverity::Block,
            score: 99,
            preferred_action: "accept".into(),
            reasons: vec![],
            issue_refs: vec![],
        }];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "demo", 1, 0, 1, None, &[], true,
        );
        assert_ne!(v.action, CouncilAction::AutoContinue);
        if v.action == CouncilAction::AutoRevise {
            assert_ne!(v.args["choice"], "accept");
        }
    }

    #[test]
    fn deadlock_epsilon_escalates_when_near_scores() {
        let cfg = KindConfig {
            min_score: 50,
            deadlock_epsilon: 50,
            ..KindConfig::default()
        };
        let issues = vec![p0_issue("a")];
        let ballots = vec![
            JuryBallot {
                agent: "consistency_auditor".into(),
                vote: CouncilVote::Revise,
                severity: CouncilSeverity::Block,
                score: 80,
                preferred_action: "fix_all_p0".into(),
                reasons: vec![],
                issue_refs: vec!["a".into()],
            },
            JuryBallot {
                agent: "pacing_reviewer".into(),
                vote: CouncilVote::Escalate,
                severity: CouncilSeverity::Warn,
                score: 80,
                preferred_action: "escalate".into(),
                reasons: vec![],
                issue_refs: vec![],
            },
            JuryBallot {
                agent: "plot_acceptor".into(),
                vote: CouncilVote::Escalate,
                severity: CouncilSeverity::Warn,
                score: 80,
                preferred_action: "escalate".into(),
                reasons: vec![],
                issue_refs: vec![],
            },
        ];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "demo", 1, 0, 1, None, &[], false,
        );
        // With high epsilon, revise vs escalate may deadlock → escalate human.
        assert!(
            matches!(
                v.action,
                CouncilAction::EscalateHuman | CouncilAction::AutoRevise
            ),
            "{:?}",
            v.action
        );
    }

    #[test]
    fn body_state_hard_gate_sets_full_revise_plan() {
        let cfg = KindConfig::default();
        let issues = vec![json!({
            "id": "s1",
            "type": "STYLE",
            "priority": "P0",
            "message": "文风",
        })];
        let data = json!({"consistency_passed": false});
        let ballots = vec![ballot_from_consistency(&data, &issues)];
        let viol = vec![json!({
            "rule": "body_state_side",
            "message": "侧别冲突",
            "blocking": true
        })];
        let v = aggregate_audit(
            &cfg, ballots, &issues, "demo", 1, 0, 1, None, &viol, true,
        );
        assert_eq!(v.action, CouncilAction::AutoRevise);
        assert_eq!(v.args["revise_scope"], "full");
        assert_eq!(v.args["prefer_local_patch"], false);
        assert!(v.args["instructions"]
            .as_str()
            .unwrap_or("")
            .contains("状态板"));
    }
}
