//! Post-plot setting pass: audit (± light sync). Does not set `volume_phase=awaiting_sync`.

use anyhow::Result;
use novelx_llm::LlmClient;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::run::PipelineEvent;
use crate::setting_audit::{
    build_setting_audit_pack, run_setting_audit, SettingAuditPackOpts, SettingAuditResult,
};
use crate::volume_sync::{run_chapter_sync, run_plot_sync, VolumeSyncReport};

#[derive(Debug, Clone)]
pub struct PlotSettingPassResult {
    pub plot_title: String,
    pub audit: SettingAuditResult,
    pub synced: bool,
    pub sync: Option<VolumeSyncReport>,
    pub report_markdown: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PlotSettingPassFlags {
    pub enabled: bool,
}

impl Default for PlotSettingPassFlags {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl PlotSettingPassFlags {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("features.yaml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        #[derive(Deserialize)]
        struct FeaturesFile {
            #[serde(default)]
            features: HashMap<String, bool>,
        }
        let Ok(file) = serde_yaml::from_str::<FeaturesFile>(&raw) else {
            return Self::default();
        };
        Self {
            enabled: file
                .features
                .get("studio.plot_setting_pass")
                .copied()
                .unwrap_or(true),
        }
    }
}

/// After each published chapter (plot still open): light entity status/holdings sync.
#[derive(Debug, Clone, Copy)]
pub struct ChapterSettingPassFlags {
    pub enabled: bool,
}

impl Default for ChapterSettingPassFlags {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl ChapterSettingPassFlags {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("features.yaml");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        #[derive(Deserialize)]
        struct FeaturesFile {
            #[serde(default)]
            features: HashMap<String, bool>,
        }
        let Ok(file) = serde_yaml::from_str::<FeaturesFile>(&raw) else {
            return Self::default();
        };
        Self {
            enabled: file
                .features
                .get("studio.chapter_setting_pass")
                .copied()
                .unwrap_or(true),
        }
    }
}

/// After a plot card completes: always audit; sync lightly unless BLOCKER or volume is ending.
pub async fn run_plot_setting_pass(
    project_dir: &Path,
    config_root: &Path,
    plot_title: &str,
    chapter: u32,
    skip_sync: bool,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<PlotSettingPassResult> {
    if let Some(t) = tx.as_ref() {
        let _ = t.send(PipelineEvent::LlmDelta {
            agent: "setting_auditor".into(),
            delta: format!("（剧情「{plot_title}」收束：设定巡检…）\n"),
        });
    }

    let pack = build_setting_audit_pack(
        project_dir,
        &SettingAuditPackOpts::post_plot(plot_title, chapter),
    );
    let audit = run_setting_audit(
        project_dir,
        config_root,
        llm.as_ref(),
        &format!("剧情卡「{plot_title}」收束后设定复核"),
        &pack,
    )
    .await?;

    let mut synced = false;
    let mut sync = None;
    if !skip_sync && audit.allows_auto_sync() {
        match run_plot_sync(
            project_dir,
            plot_title,
            chapter,
            llm.clone(),
            tx.clone(),
        )
        .await
        {
            Ok(report) => {
                synced = true;
                sync = Some(report);
            }
            Err(e) => {
                tracing::warn!(error = %e, plot = %plot_title, "plot setting sync failed");
            }
        }
    } else if audit.skipped {
        tracing::info!(
            plot = %plot_title,
            "plot setting sync skipped: setting audit did not run"
        );
    } else if audit.blocker {
        tracing::info!(
            plot = %plot_title,
            "plot setting sync skipped: setting audit BLOCKER"
        );
    } else if skip_sync {
        tracing::info!(
            plot = %plot_title,
            "plot setting sync skipped: volume end pending"
        );
    }

    let report_markdown = format_pass_report(plot_title, &audit, sync.as_ref(), skip_sync);
    Ok(PlotSettingPassResult {
        plot_title: plot_title.to_string(),
        audit,
        synced,
        sync,
        report_markdown,
    })
}

fn format_pass_report(
    plot_title: &str,
    audit: &SettingAuditResult,
    sync: Option<&VolumeSyncReport>,
    skip_sync: bool,
) -> String {
    let mut lines = vec![format!("## 剧情设定巡检\n剧情卡「{plot_title}」已收束。")];
    let audit_label = if audit.summary.is_empty() {
        if audit.skipped {
            "跳过（未实际审计）"
        } else if audit.blocker {
            "未通过（BLOCKER）"
        } else {
            "通过"
        }
    } else {
        audit.summary.as_str()
    };
    let audit_hint = if audit.skipped {
        " — 未自动同步；可稍后 `audit_setting` 再补"
    } else if audit.blocker {
        " — 已跳过自动同步，请 `audit_setting` / `design_entity` / `upsert_setting` 消解后再同步"
    } else {
        ""
    };
    lines.push(format!("- 审计：{audit_label}{audit_hint}"));
    if let Some(s) = sync {
        lines.push(format!("- 轻量同步：{}", s.message));
        if !s.incomplete_entities.is_empty() {
            lines.push(format!(
                "- 待补全 stub：{}",
                s.incomplete_entities
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
    } else if skip_sync {
        lines.push("- 轻量同步：跳过（本卷将进入卷末同步门控）".into());
    } else if audit.skipped {
        lines.push("- 轻量同步：因审计未执行而未跑".into());
    } else if audit.blocker {
        lines.push("- 轻量同步：因 BLOCKER 未执行".into());
    }
    if !audit.report.is_empty() {
        let excerpt: String = audit.report.chars().take(600).collect();
        lines.push(format!("\n<details>\n<summary>审计摘录</summary>\n\n{excerpt}\n\n</details>"));
    }
    lines.join("\n")
}

/// Light chapter-end sync (no setting audit). Used when the plot card stays open.
pub async fn run_chapter_setting_pass(
    project_dir: &Path,
    chapter: u32,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<VolumeSyncReport> {
    run_chapter_sync(project_dir, chapter, llm, tx).await
}
