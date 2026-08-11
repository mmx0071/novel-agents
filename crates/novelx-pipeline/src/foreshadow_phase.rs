//! Foreshadow pressure phase (batch brake → state machine).
//!
//! Single-chapter `continue_writing` never hard-blocks on `pressure_high` (advise only).
//! Batch soft-stops unless skipped / unattended.
//! Resolve is read-only — never write `state.json` (avoids races with publish).

use crate::memory::longform_health_snapshot;
use crate::project::{load_project_state, save_project_state};
use anyhow::Result;
use novelx_harness::LongformConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForeshadowPhase {
    Clear,
    PressureHigh,
    Paydown,
}

impl ForeshadowPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::PressureHigh => "pressure_high",
            Self::Paydown => "paydown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "clear" => Some(Self::Clear),
            "pressure_high" => Some(Self::PressureHigh),
            "paydown" => Some(Self::Paydown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ForeshadowPhaseSnapshot {
    pub phase: ForeshadowPhase,
    pub pressure: u32,
    pub total: u32,
    pub fresh: u32,
    pub far: u32,
    pub debt_cap: u32,
}

/// Explicit write (user chose paydown, etc.). Not used by resolve/status.
pub fn set_foreshadow_phase(project_dir: &Path, phase: ForeshadowPhase) -> Result<()> {
    let mut state = load_project_state(project_dir)?;
    let prev = state
        .meta
        .get("foreshadow_phase")
        .and_then(|v| v.as_str())
        .and_then(ForeshadowPhase::parse);
    if prev == Some(phase) {
        return Ok(());
    }
    state.meta.insert(
        "foreshadow_phase".into(),
        Value::String(phase.as_str().to_string()),
    );
    save_project_state(project_dir, &state)?;
    Ok(())
}

fn stored_paydown(project_dir: &Path) -> bool {
    load_project_state(project_dir)
        .ok()
        .and_then(|s| {
            s.meta
                .get("foreshadow_phase")
                .and_then(|v| v.as_str())
                .and_then(ForeshadowPhase::parse)
        })
        == Some(ForeshadowPhase::Paydown)
}

/// Resolve foreshadow phase from pressure vs cap. **Read-only** (no state write).
pub fn resolve_foreshadow_phase(
    config_root: &Path,
    project_dir: &Path,
) -> ForeshadowPhaseSnapshot {
    let lf = LongformConfig::load_from_config_root(config_root);
    let debt_cap = lf.batch_max_dangling_foreshadow;
    let health = longform_health_snapshot(project_dir);
    let pressure = health
        .pointer("/foreshadow/dangling_pressure")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let total = health
        .pointer("/foreshadow/dangling_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let fresh = health
        .pointer("/foreshadow/dangling_fresh")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let far = health
        .pointer("/foreshadow/dangling_far")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // Clear wins when under cap even if meta still says paydown.
    let phase = if debt_cap == 0 || pressure <= debt_cap {
        ForeshadowPhase::Clear
    } else if stored_paydown(project_dir) {
        ForeshadowPhase::Paydown
    } else {
        ForeshadowPhase::PressureHigh
    };

    ForeshadowPhaseSnapshot {
        phase,
        pressure,
        total,
        fresh,
        far,
        debt_cap,
    }
}

/// Batch soft-stop when pressure exceeds cap (`pressure_high` or `paydown`).
pub fn foreshadow_batch_block_message(snap: &ForeshadowPhaseSnapshot) -> Option<String> {
    match snap.phase {
        ForeshadowPhase::PressureHigh => Some(format!(
            "伏笔相位 pressure_high（压力债 {} 条 > 上限 {}；总量 {}，宽限内 {}，远期 {}）。\
             批写暂停；可先兑现近债、标 horizon=far，或 continue_writing_batch(confirm_skip_foreshadow=true) / 无人值守策略跳过。",
            snap.pressure, snap.debt_cap, snap.total, snap.fresh, snap.far
        )),
        ForeshadowPhase::Paydown => Some(format!(
            "伏笔相位 paydown（压力债 {} 条仍 > 上限 {}）。批写暂停直至近债回落；\
             或 confirm_skip_foreshadow=true / 无人值守跳过。",
            snap.pressure, snap.debt_cap
        )),
        ForeshadowPhase::Clear => None,
    }
}

/// Advice line for single-chapter continue / status (non-blocking).
pub fn foreshadow_phase_advice(snap: &ForeshadowPhaseSnapshot) -> Option<String> {
    match snap.phase {
        ForeshadowPhase::Clear => None,
        ForeshadowPhase::PressureHigh => Some(format!(
            "建议：伏笔相位 pressure_high（近债 {} > {}）。单章续写不硬拦；批写会软停。可 spawn foreshadow_tracker 或先还债。",
            snap.pressure, snap.debt_cap
        )),
        ForeshadowPhase::Paydown => Some(format!(
            "伏笔相位 paydown：优先兑现近债（当前压力 {} / 上限 {}）；批写仍软停直至回落。",
            snap.pressure, snap.debt_cap
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::init_project;
    use std::fs;

    #[test]
    fn cap_zero_is_clear() {
        let root = std::env::temp_dir().join("novelx-fsh-cap0");
        let _ = fs::remove_dir_all(&root);
        let projects = root.join("projects");
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("longform.yaml"),
            "batch_max_dangling_foreshadow: 0\n",
        )
        .unwrap();
        init_project(&projects, "sample-novel", "未定", 100).unwrap();
        let dir = projects.join("sample-novel");
        let before = load_project_state(&dir).unwrap();
        let snap = resolve_foreshadow_phase(&config, &dir);
        assert_eq!(snap.phase, ForeshadowPhase::Clear);
        assert_eq!(snap.debt_cap, 0);
        // Resolve must not rewrite state.
        let after = load_project_state(&dir).unwrap();
        assert_eq!(before.published_count, after.published_count);
        assert_eq!(before.next_chapter, after.next_chapter);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn paydown_still_blocks_batch() {
        let snap = ForeshadowPhaseSnapshot {
            phase: ForeshadowPhase::Paydown,
            pressure: 5,
            total: 8,
            fresh: 1,
            far: 2,
            debt_cap: 2,
        };
        let msg = foreshadow_batch_block_message(&snap).unwrap();
        assert!(msg.contains("paydown"));
        assert!(foreshadow_phase_advice(&snap).unwrap().contains("软停"));
    }

    #[test]
    fn pressure_high_message_and_paydown_persist() {
        let snap = ForeshadowPhaseSnapshot {
            phase: ForeshadowPhase::PressureHigh,
            pressure: 5,
            total: 8,
            fresh: 1,
            far: 2,
            debt_cap: 2,
        };
        let msg = foreshadow_batch_block_message(&snap).unwrap();
        assert!(msg.contains("pressure_high"));
        assert!(foreshadow_phase_advice(&snap).unwrap().contains("不硬拦"));

        let root = std::env::temp_dir().join("novelx-fsh-paydown");
        let _ = fs::remove_dir_all(&root);
        let projects = root.join("projects");
        fs::create_dir_all(root.join("config")).unwrap();
        init_project(&projects, "sample-novel", "未定", 100).unwrap();
        let dir = projects.join("sample-novel");
        set_foreshadow_phase(&dir, ForeshadowPhase::Paydown).unwrap();
        assert!(stored_paydown(&dir));
        let _ = fs::remove_dir_all(&root);
    }
}
