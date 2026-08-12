//! Outer-loop runtime for Goal-style batch writing (Loop Engineering).
//!
//! Persists Frame/Record/Stop beside the chapter pipeline; does **not** replace
//! in-process verify (consistency / content_rules / length).

use crate::project::project_dir;
use crate::volume_qa::{resolve_volume_qa_phase, VolumeQaPhase};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Machine-checked stop classification (string `stopped_reason` stays for clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    HardGate,
    SoftPhase,
    PublishFailed,
    Quota,
    GoalReached,
    SafetyCap,
    Cancelled,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopContract {
    pub kind: StopKind,
    /// Stable wire string (same as historical `stopped_reason`).
    pub reason: String,
    /// Auto-wake may continue only when true **and** `!requires_human` (or human armed wake).
    pub resumable: bool,
    pub requires_human: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_id: Option<String>,
}

impl StopContract {
    pub fn from_reason(reason: &str) -> Self {
        let reason = reason.trim();
        if reason.is_empty() {
            return Self {
                kind: StopKind::Completed,
                reason: "completed".into(),
                resumable: false,
                requires_human: false,
                gate_id: None,
            };
        }
        if reason.starts_with("until_chapter") {
            return Self {
                kind: StopKind::GoalReached,
                reason: reason.into(),
                resumable: false,
                requires_human: false,
                gate_id: None,
            };
        }
        if reason.starts_with("max_chapters") {
            return Self {
                kind: StopKind::Quota,
                reason: reason.into(),
                // Human may start another batch; Automations must not auto-wake.
                resumable: true,
                requires_human: true,
                gate_id: None,
            };
        }
        if reason.starts_with("safety_cap") {
            return Self {
                kind: StopKind::SafetyCap,
                reason: reason.into(),
                resumable: false,
                requires_human: true,
                gate_id: None,
            };
        }
        if reason == "completed" {
            return Self {
                kind: StopKind::Completed,
                reason: reason.into(),
                resumable: false,
                requires_human: false,
                gate_id: None,
            };
        }
        if reason == "crash_interrupted" {
            return Self {
                kind: StopKind::Cancelled,
                reason: reason.into(),
                resumable: true,
                requires_human: false,
                gate_id: None,
            };
        }

        let soft = matches!(
            reason,
            "volume_audit_gate"
                | "need_expected_review"
                | "expected_event_pending"
                | "foreshadow_pressure_high"
        );
        if soft {
            return Self {
                kind: StopKind::SoftPhase,
                reason: reason.into(),
                resumable: true,
                requires_human: true,
                gate_id: Some(reason.into()),
            };
        }

        if reason == "audit_infra" {
            return Self {
                kind: StopKind::PublishFailed,
                reason: reason.into(),
                resumable: true,
                requires_human: true,
                gate_id: Some("audit_infra".into()),
            };
        }
        let publish_fail = matches!(
            reason,
            "consistency_fail"
                | "content_rule"
                | "draft_exists"
                | "needs_user_choice"
                | "not_published"
                | "gate"
        ) || reason.starts_with("soft_short")
            || reason.starts_with("hard_short")
            || reason.starts_with("hard_long");
        if publish_fail {
            return Self {
                kind: StopKind::PublishFailed,
                reason: reason.into(),
                resumable: true,
                requires_human: true,
                gate_id: Some(reason.into()),
            };
        }

        // Hard phase / order / plot / volume end.
        let gate_id = if reason.starts_with("plot_gate:") {
            Some(reason.to_string())
        } else if reason.starts_with("volume_phase:") || reason.starts_with("setup_phase:") {
            Some(reason.to_string())
        } else {
            Some(reason.to_string())
        };
        Self {
            kind: StopKind::HardGate,
            reason: reason.into(),
            resumable: true,
            requires_human: true,
            gate_id,
        }
    }

    /// Automations may call LoopRunner without a human click.
    pub fn auto_wake_allowed(&self) -> bool {
        self.resumable && !self.requires_human
    }

    pub fn status_label_zh(&self) -> &'static str {
        if self.auto_wake_allowed() {
            return "可续写";
        }
        if self.requires_human {
            return "需处理";
        }
        match self.kind {
            StopKind::GoalReached | StopKind::Completed => "已完成",
            StopKind::Quota => "达批上限",
            _ => "已停止",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopJobStatus {
    Running,
    Stopped,
    Armed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopJob {
    pub project: String,
    pub status: LoopJobStatus,
    pub until_chapter: Option<u32>,
    pub max_chapters: Option<u32>,
    #[serde(default)]
    pub respect_soft_gates: bool,
    #[serde(default)]
    pub confirm_skip_volume_audit: bool,
    #[serde(default)]
    pub confirm_skip_expected: bool,
    #[serde(default)]
    pub confirm_skip_foreshadow: bool,
    #[serde(default = "default_true")]
    pub auto_length_revise: bool,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub chapters_done: u32,
    #[serde(default)]
    pub chapters_attempted: u32,
    #[serde(default)]
    pub started_chapter: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop: Option<StopContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub soft_gates_skipped: Vec<String>,
    /// Human cleared a blocking gate; wake loop may resume once.
    #[serde(default)]
    pub pending_wake: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end_verify: Option<LoopEndVerify>,
    #[serde(default)]
    pub unit: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopEndVerify {
    pub ran: bool,
    pub requires_human: bool,
    pub volume_qa_phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

pub fn loop_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".novelx").join("loop")
}

pub fn job_path(project_dir: &Path) -> PathBuf {
    loop_dir(project_dir).join("job.json")
}

pub fn journal_path(project_dir: &Path) -> PathBuf {
    loop_dir(project_dir).join("journal.jsonl")
}

pub fn ensure_loop_dir(project_dir: &Path) -> Result<()> {
    fs::create_dir_all(loop_dir(project_dir)).context("create .novelx/loop")?;
    Ok(())
}

pub fn load_loop_job(project_dir: &Path) -> Option<LoopJob> {
    let path = job_path(project_dir);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_loop_job(project_dir: &Path, job: &LoopJob) -> Result<()> {
    ensure_loop_dir(project_dir)?;
    let path = job_path(project_dir);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(job)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn append_loop_journal(project_dir: &Path, entry: Value) -> Result<()> {
    ensure_loop_dir(project_dir)?;
    let path = journal_path(project_dir);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("open journal.jsonl")?;
    let mut line = entry.to_string();
    line.push('\n');
    f.write_all(line.as_bytes())?;
    Ok(())
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn heartbeat_loop_job(project_dir: &Path) -> Result<()> {
    if let Some(mut job) = load_loop_job(project_dir) {
        if matches!(job.status, LoopJobStatus::Running) {
            job.updated_at = now_rfc3339();
            save_loop_job(project_dir, &job)?;
        }
    }
    Ok(())
}

/// Mark that a human cleared the blocking gate so Automations may resume once.
pub fn arm_loop_wake_after_human(project_dir: &Path) -> Result<bool> {
    let Some(mut job) = load_loop_job(project_dir) else {
        return Ok(false);
    };
    let Some(stop) = job.last_stop.clone() else {
        return Ok(false);
    };
    if !stop.resumable || !stop.requires_human {
        return Ok(false);
    }
    job.pending_wake = true;
    job.status = LoopJobStatus::Armed;
    job.updated_at = now_rfc3339();
    // Human armed → allow one auto wake even though requires_human was set.
    save_loop_job(project_dir, &job)?;
    append_loop_journal(
        project_dir,
        json!({
            "ts": now_rfc3339(),
            "event": "human_gate_cleared",
            "reason": stop.reason,
        }),
    )?;
    Ok(true)
}

/// Soft-skip policy keys applied for this batch (for journal / UI).
pub fn soft_skip_keys_from_opts(
    applied_by_policy: bool,
    skip_volume: bool,
    skip_expected: bool,
    skip_foreshadow: bool,
) -> Vec<String> {
    if !applied_by_policy {
        return Vec::new();
    }
    let mut keys = Vec::new();
    if skip_volume {
        keys.push("batch.skip_volume_audit_mid".into());
    }
    if skip_expected {
        keys.push("batch.skip_expected_review".into());
    }
    if skip_foreshadow {
        keys.push("batch.skip_foreshadow_pressure".into());
    }
    keys
}

pub fn begin_loop_job(
    project_dir: &Path,
    project: &str,
    opts: &crate::batch::BatchContinueOpts,
    started_chapter: u32,
    unit: &str,
    soft_gates_skipped: Vec<String>,
) -> Result<LoopJob> {
    let now = now_rfc3339();
    let job = LoopJob {
        project: project.to_string(),
        status: LoopJobStatus::Running,
        until_chapter: opts.until_chapter,
        max_chapters: opts.max_chapters,
        respect_soft_gates: opts.respect_soft_gates,
        confirm_skip_volume_audit: opts.confirm_skip_volume_audit,
        confirm_skip_expected: opts.confirm_skip_expected,
        confirm_skip_foreshadow: opts.confirm_skip_foreshadow,
        auto_length_revise: opts.auto_length_revise,
        started_at: now.clone(),
        updated_at: now.clone(),
        chapters_done: 0,
        chapters_attempted: 0,
        started_chapter,
        last_stop: None,
        soft_gates_skipped: soft_gates_skipped.clone(),
        pending_wake: false,
        loop_end_verify: None,
        unit: unit.to_string(),
    };
    save_loop_job(project_dir, &job)?;
    append_loop_journal(
        project_dir,
        json!({
            "ts": now,
            "event": "loop_begin",
            "project": project,
            "started_chapter": started_chapter,
            "until_chapter": opts.until_chapter,
            "max_chapters": opts.max_chapters,
            "soft_gates_skipped": soft_gates_skipped,
        }),
    )?;
    Ok(job)
}

pub fn finish_loop_job(
    project_dir: &Path,
    mut job: LoopJob,
    stop: StopContract,
    chapters_done: u32,
    chapters_attempted: u32,
    verify: Option<LoopEndVerify>,
) -> Result<LoopJob> {
    let mut stop = stop;
    if let Some(ref v) = verify {
        if v.requires_human {
            stop.requires_human = true;
            // Block auto-wake after quality flag.
            job.pending_wake = false;
        }
    }
    job.status = LoopJobStatus::Stopped;
    job.last_stop = Some(stop.clone());
    job.chapters_done = chapters_done;
    job.chapters_attempted = chapters_attempted;
    job.updated_at = now_rfc3339();
    job.loop_end_verify = verify.clone();
    job.pending_wake = false;
    save_loop_job(project_dir, &job)?;
    append_loop_journal(
        project_dir,
        json!({
            "ts": job.updated_at,
            "event": "loop_stop",
            "stop": stop,
            "chapters_done": chapters_done,
            "chapters_attempted": chapters_attempted,
            "loop_end_verify": verify,
            "soft_gates_skipped": job.soft_gates_skipped,
        }),
    )?;
    Ok(job)
}

/// Deterministic loop-end verify (volume QA phase). Short drama / below threshold → skip.
pub fn run_loop_end_verify(
    config_root: &Path,
    project_dir: &Path,
    chapters_published: u32,
    min_chapters: u32,
    stop: &StopContract,
    is_short_drama: bool,
) -> Option<LoopEndVerify> {
    if is_short_drama || min_chapters == 0 || chapters_published < min_chapters {
        return None;
    }
    if !matches!(
        stop.kind,
        StopKind::GoalReached | StopKind::Quota | StopKind::Completed
    ) {
        return None;
    }
    let state = crate::project::load_project_state(project_dir).ok()?;
    let chapter = state.next_chapter.max(1).saturating_sub(1).max(1);
    let phase = resolve_volume_qa_phase(config_root, project_dir, chapter);
    let requires_human = matches!(
        phase,
        VolumeQaPhase::MidDue | VolumeQaPhase::HandoffRequired
    );
    let summary = match phase {
        VolumeQaPhase::Ok => Some("卷 QA 相位 ok；批结束跨章抽检通过（确定性）".into()),
        VolumeQaPhase::MidDue => Some(
            "卷 QA 相位 mid_due：建议先 audit_volume 再继续连写（已阻断自动唤醒）".into(),
        ),
        VolumeQaPhase::HandoffRequired => Some(
            "卷 QA 相位 handoff_required：卷末交接审未完成（已阻断自动唤醒）".into(),
        ),
    };
    Some(LoopEndVerify {
        ran: true,
        requires_human,
        volume_qa_phase: phase.as_str().to_string(),
        summary,
    })
}

/// Whether Automations should resume this project's loop.
pub fn should_auto_wake(job: &LoopJob, stale_running_secs: u64) -> bool {
    if job.pending_wake {
        if let Some(stop) = &job.last_stop {
            return stop.resumable;
        }
        return true;
    }
    if matches!(job.status, LoopJobStatus::Running) {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&job.updated_at) {
            let age = Utc::now().signed_duration_since(ts.with_timezone(&Utc));
            if age.num_seconds() >= stale_running_secs as i64 {
                return true;
            }
        }
    }
    if let Some(stop) = &job.last_stop {
        return stop.auto_wake_allowed();
    }
    false
}

pub fn loop_status_dto(project_dir: &Path) -> Value {
    match load_loop_job(project_dir) {
        Some(job) => {
            let stop = job.last_stop.clone();
            let label = stop
                .as_ref()
                .map(|s| s.status_label_zh())
                .unwrap_or(match job.status {
                    LoopJobStatus::Running => "运行中",
                    LoopJobStatus::Armed => "已武装待续",
                    LoopJobStatus::Stopped => "已停止",
                });
            json!({
                "ok": true,
                "hasJob": true,
                "project": job.project,
                "status": job.status,
                "statusLabel": label,
                "startedChapter": job.started_chapter,
                "chaptersDone": job.chapters_done,
                "chaptersAttempted": job.chapters_attempted,
                "untilChapter": job.until_chapter,
                "maxChapters": job.max_chapters,
                "softGatesSkipped": job.soft_gates_skipped,
                "pendingWake": job.pending_wake,
                "lastStop": stop,
                "loopEndVerify": job.loop_end_verify,
                "unit": job.unit,
                "updatedAt": job.updated_at,
                "resumable": stop.as_ref().map(|s| s.resumable).unwrap_or(false),
                "requiresHuman": stop.as_ref().map(|s| s.requires_human).unwrap_or(false)
                    || job.loop_end_verify.as_ref().map(|v| v.requires_human).unwrap_or(false),
            })
        }
        None => json!({
            "ok": true,
            "hasJob": false,
        }),
    }
}

/// Rebuild BatchContinueOpts from a persisted job (crash / armed resume).
pub fn opts_from_job(job: &LoopJob) -> crate::batch::BatchContinueOpts {
    crate::batch::BatchContinueOpts {
        project: job.project.clone(),
        max_chapters: job.max_chapters,
        until_chapter: job.until_chapter,
        confirm_skip_volume_audit: job.confirm_skip_volume_audit,
        confirm_skip_expected: job.confirm_skip_expected,
        confirm_skip_foreshadow: job.confirm_skip_foreshadow,
        respect_soft_gates: job.respect_soft_gates,
        auto_length_revise: job.auto_length_revise,
    }
}

pub fn list_projects_with_loop_jobs(projects_root: &Path) -> Vec<(String, PathBuf, LoopJob)> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(projects_root) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let dir = project_dir(projects_root, &name);
        if let Some(job) = load_loop_job(&dir) {
            out.push((name, dir, job));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_contract_hard_gate_requires_human() {
        let c = StopContract::from_reason("chapter_order");
        assert_eq!(c.kind, StopKind::HardGate);
        assert!(c.requires_human);
        assert!(!c.auto_wake_allowed());
        assert_eq!(c.status_label_zh(), "需处理");
    }

    #[test]
    fn stop_contract_goal_and_quota() {
        let g = StopContract::from_reason("until_chapter(10)");
        assert_eq!(g.kind, StopKind::GoalReached);
        assert!(!g.requires_human);
        assert!(!g.auto_wake_allowed());

        let q = StopContract::from_reason("max_chapters(20)");
        assert_eq!(q.kind, StopKind::Quota);
        assert!(q.requires_human);
        assert!(!q.auto_wake_allowed());
    }

    #[test]
    fn stop_contract_crash_auto_wake() {
        let c = StopContract::from_reason("crash_interrupted");
        assert!(c.auto_wake_allowed());
    }

    #[test]
    fn soft_skip_keys_recorded() {
        let keys = soft_skip_keys_from_opts(true, true, true, false);
        assert_eq!(
            keys,
            vec![
                "batch.skip_volume_audit_mid".to_string(),
                "batch.skip_expected_review".to_string(),
            ]
        );
        assert!(soft_skip_keys_from_opts(false, true, true, true).is_empty());
    }

    #[test]
    fn loop_end_verify_blocks_wake_on_mid_due() {
        let stop = StopContract::from_reason("max_chapters(5)");
        assert!(matches!(stop.kind, StopKind::Quota));
        // Below threshold → None
        let none = run_loop_end_verify(
            Path::new("/nonexistent"),
            Path::new("/nonexistent"),
            2,
            5,
            &stop,
            false,
        );
        assert!(none.is_none());
        let skip_sd = run_loop_end_verify(
            Path::new("/nonexistent"),
            Path::new("/nonexistent"),
            10,
            5,
            &stop,
            true,
        );
        assert!(skip_sd.is_none());
    }

    #[test]
    fn job_persist_roundtrip() {
        let root = std::env::temp_dir().join(format!(
            "novelx-loop-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let dir = root.join("sample-novel");
        fs::create_dir_all(&dir).unwrap();
        let opts = crate::batch::BatchContinueOpts {
            project: "sample-novel".into(),
            max_chapters: Some(3),
            until_chapter: Some(10),
            ..Default::default()
        };
        let job = begin_loop_job(
            &dir,
            "sample-novel",
            &opts,
            1,
            "章",
            vec!["batch.skip_volume_audit_mid".into()],
        )
        .unwrap();
        assert!(matches!(job.status, LoopJobStatus::Running));
        let stop = StopContract::from_reason("consistency_fail");
        let finished = finish_loop_job(&dir, job, stop, 0, 1, None).unwrap();
        assert!(matches!(finished.status, LoopJobStatus::Stopped));
        assert!(finished.last_stop.unwrap().requires_human);
        assert!(arm_loop_wake_after_human(&dir).unwrap());
        let armed = load_loop_job(&dir).unwrap();
        assert!(armed.pending_wake);
        assert!(should_auto_wake(&armed, 600));
        let _ = fs::remove_dir_all(&root);
    }
}
