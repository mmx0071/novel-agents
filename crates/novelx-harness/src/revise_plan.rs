//! Deterministic RevisePlan — classify audit fail / hard gates into local|full|escalate
//! before `steer_run` / `revise_chapter`. Config: `decision_council.yaml` → `revise_plan`.

use crate::{issue_priority, issue_type};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

static REVISE_PLAN_CACHE: RwLock<Option<(PathBuf, Option<SystemTime>, RevisePlanConfig)>> =
    RwLock::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviseScope {
    Local,
    Full,
    Escalate,
}

impl ReviseScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Full => "full",
            Self::Escalate => "escalate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisePlan {
    pub scope: ReviseScope,
    #[serde(default)]
    pub issue_ids: Vec<String>,
    #[serde(default)]
    pub hard_gates: Vec<String>,
    pub instructions: String,
    pub rationale: String,
    /// Explicit flag for `revise_chapter` (prefer over keyword sniffing).
    pub prefer_local_patch: bool,
}

impl RevisePlan {
    pub fn to_value(&self) -> Value {
        json!({
            "scope": self.scope.as_str(),
            "issue_ids": self.issue_ids,
            "hard_gates": self.hard_gates,
            "instructions": self.instructions,
            "rationale": self.rationale,
            "prefer_local_patch": self.prefer_local_patch,
        })
    }

    pub fn summary_line(&self) -> String {
        let n = self.issue_ids.len();
        let gates = if self.hard_gates.is_empty() {
            String::new()
        } else {
            format!(" · {}", self.hard_gates.join("+"))
        };
        match self.scope {
            ReviseScope::Full => format!("整章修订{gates} · {n} 条 issue"),
            ReviseScope::Local => format!("局部修订{gates} · {n} 条 issue"),
            ReviseScope::Escalate => format!("升人机{gates} · {}", self.rationale),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CouncilFileSlice {
    #[serde(default)]
    revise_plan: Option<RevisePlanConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RevisePlanConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_full_p0_types")]
    pub full_scope_p0_types: Vec<String>,
    #[serde(default = "default_full_hard_rules")]
    pub full_scope_hard_rules: Vec<String>,
    /// When same-type streak reaches this, promote local → full.
    #[serde(default = "default_promote_streak")]
    pub promote_to_full_after_streak: u32,
}

fn default_true() -> bool {
    true
}

fn default_full_p0_types() -> Vec<String> {
    vec![
        "TIMELINE".into(),
        "INJURY".into(),
        "CONTINUITY".into(),
        "BODY".into(),
        "ABILITY_LOC".into(),
    ]
}

fn default_full_hard_rules() -> Vec<String> {
    vec![
        "body_state_side".into(),
        "body_state_locus".into(),
        "timeline_daypart_regression".into(),
        "timeline_countdown_jump".into(),
    ]
}

fn default_promote_streak() -> u32 {
    1
}

impl Default for RevisePlanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            full_scope_p0_types: default_full_p0_types(),
            full_scope_hard_rules: default_full_hard_rules(),
            promote_to_full_after_streak: default_promote_streak(),
        }
    }
}

impl RevisePlanConfig {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("decision_council.yaml");
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();
        if let Ok(guard) = REVISE_PLAN_CACHE.read() {
            if let Some((ref cached_path, ref cached_mtime, ref cfg)) = *guard {
                if cached_path == &path && cached_mtime == &mtime {
                    return cfg.clone();
                }
            }
        }
        let cfg = Self::load_uncached(&path);
        if let Ok(mut guard) = REVISE_PLAN_CACHE.write() {
            *guard = Some((path, mtime, cfg.clone()));
        }
        cfg
    }

    fn load_uncached(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(file) = serde_yaml::from_str::<CouncilFileSlice>(&raw) else {
            return Self::default();
        };
        file.revise_plan.unwrap_or_default()
    }
}

/// Collect blocking hard-rule ids from pipeline/audit `content_rule_violations`.
pub fn hard_gate_ids_from_violations(violations: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = violations
        .iter()
        .filter(|v| v.get("blocking").and_then(|b| b.as_bool()).unwrap_or(true))
        .filter_map(|v| {
            v.get("rule")
                .and_then(|r| r.as_str())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Build a deterministic revise plan. Genre-neutral: types + rule ids only.
pub fn build_revise_plan(
    cfg: &RevisePlanConfig,
    issues: &[Value],
    violations: &[Value],
    same_type_streak: u32,
) -> RevisePlan {
    if !cfg.enabled {
        return legacy_local_plan(issues);
    }

    let hard_gates = hard_gate_ids_from_violations(violations);
    let p0: Vec<&Value> = issues
        .iter()
        .filter(|i| issue_priority(i) == "P0")
        .collect();
    let issue_ids = collect_issue_ids(issues, !p0.is_empty());

    let full_by_hard = hard_gates.iter().any(|g| rule_forces_full_scope(cfg, g));

    let p0_types: Vec<String> = p0.iter().map(|i| issue_type(i)).collect();
    let full_by_type = p0_types.iter().any(|t| {
        cfg.full_scope_p0_types
            .iter()
            .any(|x| x.eq_ignore_ascii_case(t))
    });

    let only_meta = !p0.is_empty()
        && p0.iter().all(|i| issue_type(i).eq_ignore_ascii_case("META"))
        && hard_gates.is_empty();

    let mut scope = if full_by_hard || (full_by_type && !only_meta) {
        ReviseScope::Full
    } else if p0.is_empty() && hard_gates.is_empty() && issue_ids.is_empty() {
        ReviseScope::Escalate
    } else {
        ReviseScope::Local
    };

    // Promote sticky same-type rounds to full (one shot before streak escalate).
    // META-only stays local — quote thrash on chapter-number noise must not force rewrite.
    if scope == ReviseScope::Local
        && !p0.is_empty()
        && !only_meta
        && same_type_streak >= cfg.promote_to_full_after_streak.max(1)
    {
        scope = ReviseScope::Full;
    }

    if scope == ReviseScope::Escalate {
        return RevisePlan {
            scope,
            issue_ids,
            hard_gates,
            instructions: String::new(),
            rationale: "无可执行修订信号".into(),
            prefer_local_patch: true,
        };
    }

    let prefer_local = scope == ReviseScope::Local;
    let instructions = build_instructions(scope, &hard_gates, issues, &p0);
    let rationale = match scope {
        ReviseScope::Full if full_by_hard => {
            format!("硬门 {} → 整章对齐", hard_gates.join("+"))
        }
        ReviseScope::Full if full_by_type => "连续性/时间线类 P0 → 整章修订".into(),
        ReviseScope::Full => "同类未消或需整章落地".into(),
        ReviseScope::Local => "可局部修订".into(),
        ReviseScope::Escalate => "升人机".into(),
    };

    RevisePlan {
        scope,
        issue_ids,
        hard_gates,
        instructions,
        rationale,
        prefer_local_patch: prefer_local,
    }
}

fn legacy_local_plan(issues: &[Value]) -> RevisePlan {
    let p0_any = issues.iter().any(|i| issue_priority(i) == "P0");
    let issue_ids = collect_issue_ids(issues, p0_any);
    RevisePlan {
        scope: ReviseScope::Local,
        issue_ids: issue_ids.clone(),
        hard_gates: vec![],
        instructions: "按最近一致性审计意见局部修订".into(),
        rationale: "revise_plan 未启用".into(),
        prefer_local_patch: true,
    }
}

fn collect_issue_ids(issues: &[Value], p0_only: bool) -> Vec<String> {
    issues
        .iter()
        .filter(|i| !p0_only || issue_priority(i) == "P0")
        .filter_map(|i| i.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect()
}

fn build_instructions(
    scope: ReviseScope,
    hard_gates: &[String],
    issues: &[Value],
    p0: &[&Value],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    match scope {
        ReviseScope::Full => {
            parts.push(
                "整章修订（非局部补丁）：通读本章后按下列约束改写相关段落，保持情节推进。"
                    .into(),
            );
            if hard_gates.iter().any(|g| g.contains("body_state")) {
                parts.push(
                    "硬门：正文侧别/伤情载体必须与状态板一致；禁止只改单句 quote 敷衍；不得把伤情翻到对侧。"
                        .into(),
                );
            }
            if hard_gates
                .iter()
                .any(|g| !g.contains("body_state"))
            {
                let other: Vec<&str> = hard_gates
                    .iter()
                    .filter(|g| !g.contains("body_state"))
                    .map(|s| s.as_str())
                    .collect();
                if !other.is_empty() {
                    parts.push(format!("同时消除硬规则：{}。", other.join("、")));
                }
            }
        }
        ReviseScope::Local => {
            parts.push("按下列审校问题局部修订（仅这些项）。".into());
        }
        ReviseScope::Escalate => return String::new(),
    }

    let brief = format_issue_bullets(if p0.is_empty() {
        issues.iter().collect::<Vec<_>>()
    } else {
        p0.to_vec()
    });
    if !brief.is_empty() {
        parts.push(brief);
    }
    parts.join("\n\n")
}

fn format_issue_bullets(issues: Vec<&Value>) -> String {
    let mut lines = Vec::new();
    for (idx, i) in issues.iter().take(12).enumerate() {
        let ty = issue_type(i);
        let id = i.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let msg = i
            .get("message")
            .or_else(|| i.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();
        let quote = i
            .get("quote")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();
        let loc = i
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut line = format!("{}. [{ty}]", idx + 1);
        if !id.is_empty() {
            line.push_str(&format!(" id={id}"));
        }
        if !loc.is_empty() {
            line.push_str(&format!(" @{loc}"));
        }
        if !msg.is_empty() {
            line.push_str(&format!(" {msg}"));
        }
        if !quote.is_empty() {
            line.push_str(&format!(" 「{quote}」"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn rule_forces_full_scope(cfg: &RevisePlanConfig, rule: &str) -> bool {
    let lower = rule.to_ascii_lowercase();
    cfg.full_scope_hard_rules
        .iter()
        .any(|r| r.eq_ignore_ascii_case(rule))
        || lower.starts_with("body_state")
        || lower.starts_with("timeline_")
}

/// Merge plan fields into `steer_run` / `revise_chapter` args.
/// Does not overwrite caller `issue_ids` when already present (per-issue gate buttons).
pub fn apply_plan_to_steer_args(args: &mut Value, plan: &RevisePlan) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };
    let has_ids = obj
        .get("issue_ids")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty());
    if !has_ids && !plan.issue_ids.is_empty() {
        obj.insert("issue_ids".into(), json!(plan.issue_ids));
    }
    if !plan.instructions.is_empty() {
        // Prefer plan instructions when caller left them empty.
        let existing = obj
            .get("instructions")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if existing.is_empty() {
            obj.insert("instructions".into(), json!(plan.instructions));
        }
    }
    obj.insert("revise_scope".into(), json!(plan.scope.as_str()));
    obj.insert("prefer_local_patch".into(), json!(plan.prefer_local_patch));
    obj.insert("revise_plan".into(), plan.to_value());
}

/// Persist council same-type streak so human `steer_run` rebuild can promote scope.
pub fn persist_revise_context(
    project_dir: &Path,
    chapter: u32,
    same_type_streak: u32,
    p0_types: &[String],
) {
    let dir = project_dir.join("chapters").join(format!("{chapter:03}"));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("revise_plan_ctx.json");
    let body = json!({
        "chapter": chapter,
        "same_type_streak": same_type_streak,
        "p0_types": p0_types,
    });
    let _ = std::fs::write(path, body.to_string());
}

/// Load persisted streak for this chapter (0 if missing / other chapter).
pub fn load_revise_streak(project_dir: &Path, chapter: u32) -> u32 {
    let path = project_dir
        .join("chapters")
        .join(format!("{chapter:03}"))
        .join("revise_plan_ctx.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return 0;
    };
    if v.get("chapter").and_then(|c| c.as_u64()) != Some(chapter as u64) {
        return 0;
    }
    v.get("same_type_streak")
        .and_then(|s| s.as_u64())
        .unwrap_or(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p0(id: &str, ty: &str) -> Value {
        json!({
            "id": id,
            "type": ty,
            "priority": "P0",
            "message": "test",
            "quote": "q"
        })
    }

    fn meta(id: &str) -> Value {
        json!({
            "id": id,
            "type": "META",
            "priority": "P1",
            "message": "软噪"
        })
    }

    #[test]
    fn meta_soft_is_local() {
        let cfg = RevisePlanConfig::default();
        let plan = build_revise_plan(&cfg, &[meta("m1")], &[], 0);
        assert_eq!(plan.scope, ReviseScope::Local);
        assert!(plan.prefer_local_patch);
    }

    #[test]
    fn body_state_side_forces_full() {
        let cfg = RevisePlanConfig::default();
        let viol = json!({"rule": "body_state_side", "message": "侧别冲突", "blocking": true});
        let plan = build_revise_plan(&cfg, &[p0("a", "STYLE")], &[viol], 0);
        assert_eq!(plan.scope, ReviseScope::Full);
        assert!(!plan.prefer_local_patch);
        assert!(plan.instructions.contains("状态板"));
        assert!(plan.hard_gates.iter().any(|g| g == "body_state_side"));
    }

    #[test]
    fn timeline_p0_forces_full() {
        let cfg = RevisePlanConfig::default();
        let plan = build_revise_plan(&cfg, &[p0("t1", "TIMELINE")], &[], 0);
        assert_eq!(plan.scope, ReviseScope::Full);
        assert!(!plan.prefer_local_patch);
    }

    #[test]
    fn streak_promotes_other_p0_to_full() {
        let cfg = RevisePlanConfig {
            promote_to_full_after_streak: 1,
            ..RevisePlanConfig::default()
        };
        let plan = build_revise_plan(&cfg, &[p0("s1", "STYLE")], &[], 1);
        assert_eq!(plan.scope, ReviseScope::Full);
    }

    #[test]
    fn meta_p0_not_promoted_by_streak() {
        let cfg = RevisePlanConfig {
            promote_to_full_after_streak: 1,
            ..RevisePlanConfig::default()
        };
        let plan = build_revise_plan(
            &cfg,
            &[json!({
                "id": "m1",
                "type": "META",
                "priority": "P0",
                "message": "章号元叙述"
            })],
            &[],
            2,
        );
        assert_eq!(plan.scope, ReviseScope::Local);
    }

    #[test]
    fn timeline_hard_rule_forces_full() {
        let cfg = RevisePlanConfig::default();
        let viol = json!({
            "rule": "timeline_daypart_regression",
            "message": "时段回跳",
            "blocking": true
        });
        let plan = build_revise_plan(&cfg, &[], &[viol], 0);
        assert_eq!(plan.scope, ReviseScope::Full);
    }

    #[test]
    fn apply_plan_keeps_caller_issue_ids() {
        let cfg = RevisePlanConfig::default();
        let plan = build_revise_plan(&cfg, &[p0("t1", "TIMELINE"), p0("t2", "TIMELINE")], &[], 0);
        let mut args = json!({
            "project": "demo",
            "chapter": 1,
            "choice": "revise",
            "issue_ids": ["t1"]
        });
        apply_plan_to_steer_args(&mut args, &plan);
        assert_eq!(args["issue_ids"], json!(["t1"]));
        assert_eq!(args["revise_scope"], "full");
    }

    #[test]
    fn other_p0_stays_local_when_streak_zero() {
        let cfg = RevisePlanConfig {
            promote_to_full_after_streak: 2,
            ..RevisePlanConfig::default()
        };
        let plan = build_revise_plan(&cfg, &[p0("s1", "STYLE")], &[], 0);
        assert_eq!(plan.scope, ReviseScope::Local);
    }

    #[test]
    fn empty_signals_escalate() {
        let cfg = RevisePlanConfig::default();
        let plan = build_revise_plan(&cfg, &[], &[], 0);
        assert_eq!(plan.scope, ReviseScope::Escalate);
    }

    #[test]
    fn apply_plan_sets_steer_fields() {
        let cfg = RevisePlanConfig::default();
        let plan = build_revise_plan(&cfg, &[p0("t1", "TIMELINE")], &[], 0);
        let mut args = json!({"project": "demo", "chapter": 1, "choice": "revise"});
        apply_plan_to_steer_args(&mut args, &plan);
        assert_eq!(args["revise_scope"], "full");
        assert_eq!(args["prefer_local_patch"], false);
        assert!(!args["instructions"].as_str().unwrap_or("").is_empty());
    }
}
