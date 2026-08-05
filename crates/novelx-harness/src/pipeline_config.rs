//! Chapter pipeline order / MVP / step handlers from `config/pipeline.yaml`.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Stable handler kinds — new polish agents can reuse `SpecialistRewrite` via YAML only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerKind {
    ChapterPlanner,
    LoreQuery,
    Writer,
    Nomenclature,
    SpecialistRewrite,
    Consistency,
    Pacing,
    Foreshadow,
    Summarizer,
    PlotAccept,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HandlerSpec {
    pub kind: HandlerKind,
    #[serde(default)]
    pub focus: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PipelineFile {
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    mvp: Vec<String>,
    #[serde(default)]
    audit_only: Vec<String>,
    #[serde(default)]
    revise_default: Vec<String>,
    #[serde(default)]
    handlers: HashMap<String, HandlerSpec>,
}

/// Loaded pipeline schedule — single source of truth for chapter agent order + handlers.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    inner: Arc<PipelineFile>,
}

impl PipelineConfig {
    pub fn load(config_root: &Path) -> Self {
        Self::load_for_mode(config_root, "longform")
    }

    /// Load `pipeline.yaml` (longform) or `pipeline-script.yaml` (short_drama).
    pub fn load_for_mode(config_root: &Path, mode: &str) -> Self {
        let file_name = match mode {
            "short_drama" | "short-drama" | "script" => "pipeline-script.yaml",
            _ => "pipeline.yaml",
        };
        let path = config_root.join(file_name);
        if !path.exists() {
            tracing::warn!(
                path = %path.display(),
                mode,
                "pipeline config missing; using embedded defaults for mode"
            );
            return Self::defaults_for_mode(mode);
        }
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_yaml::from_str::<PipelineFile>(&raw) {
                Ok(file) if !file.order.is_empty() => {
                    tracing::info!(
                        order = file.order.len(),
                        mvp = file.mvp.len(),
                        handlers = file.handlers.len(),
                        mode,
                        file = file_name,
                        "pipeline config loaded"
                    );
                    Self {
                        inner: Arc::new(normalize_for_mode(file, mode)),
                    }
                }
                Ok(_) => {
                    tracing::warn!(file = file_name, "pipeline has empty order; using defaults");
                    Self::defaults_for_mode(mode)
                }
                Err(e) => {
                    tracing::warn!(error = %e, file = file_name, "pipeline parse failed; using defaults");
                    Self::defaults_for_mode(mode)
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, file = file_name, "pipeline read failed; using defaults");
                Self::defaults_for_mode(mode)
            }
        }
    }

    pub fn defaults() -> Self {
        Self::defaults_for_mode("longform")
    }

    pub fn defaults_for_mode(mode: &str) -> Self {
        let yaml = match mode {
            "short_drama" | "short-drama" | "script" => {
                include_str!("../../../config/pipeline-script.yaml")
            }
            _ => include_str!("../../../config/pipeline.yaml"),
        };
        let file: PipelineFile =
            serde_yaml::from_str(yaml).expect("embedded pipeline yaml must parse");
        Self {
            inner: Arc::new(normalize_for_mode(file, mode)),
        }
    }

    pub fn order(&self) -> &[String] {
        &self.inner.order
    }

    pub fn mvp(&self) -> &[String] {
        &self.inner.mvp
    }

    pub fn audit_only(&self) -> &[String] {
        &self.inner.audit_only
    }

    pub fn revise_default(&self) -> &[String] {
        &self.inner.revise_default
    }

    pub fn is_pipeline_agent(&self, role: &str) -> bool {
        self.inner.order.iter().any(|a| a == role)
    }

    pub fn handler_for(&self, agent: &str) -> Option<&HandlerSpec> {
        self.inner.handlers.get(agent)
    }
}

fn default_handlers_for_mode(mode: &str) -> HashMap<String, HandlerSpec> {
    let yaml = match mode {
        "short_drama" | "short-drama" | "script" => {
            include_str!("../../../config/pipeline-script.yaml")
        }
        _ => include_str!("../../../config/pipeline.yaml"),
    };
    let file: PipelineFile =
        serde_yaml::from_str(yaml).expect("embedded pipeline yaml must parse");
    file.handlers
}

fn normalize_for_mode(mut file: PipelineFile, mode: &str) -> PipelineFile {
    let short = matches!(mode, "short_drama" | "short-drama" | "script");
    if file.audit_only.is_empty() {
        file.audit_only = vec![
            "consistency_auditor".into(),
            "pacing_reviewer".into(),
        ];
    }
    if file.revise_default.is_empty() {
        file.revise_default = if short {
            vec![
                "script_writer".into(),
                "consistency_auditor".into(),
                "pacing_reviewer".into(),
                "summarizer".into(),
                "beat_acceptor".into(),
            ]
        } else {
            vec![
                "writer".into(),
                "consistency_auditor".into(),
                "pacing_reviewer".into(),
                "summarizer".into(),
                "plot_acceptor".into(),
            ]
        };
    }
    if file.mvp.is_empty() {
        file.mvp = file.order.iter().take(7).cloned().collect();
    }
    // Merge defaults under user entries so a partial `handlers:` block cannot
    // silently drop writer/summarizer/etc. (user keys always win).
    let defaults = default_handlers_for_mode(mode);
    if file.handlers.is_empty() {
        file.handlers = defaults;
    } else {
        for (k, v) in defaults {
            file.handlers.entry(k).or_insert(v);
        }
    }
    for a in &file.order {
        if !file.handlers.contains_key(a) {
            tracing::warn!(
                agent = %a,
                "pipeline order agent has no handler after merge"
            );
        }
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_writer_and_mvp() {
        let p = PipelineConfig::defaults();
        assert!(p.is_pipeline_agent("writer"));
        assert!(p.mvp().iter().any(|a| a == "writer"));
        assert!(p.order().iter().any(|a| a == "lore_librarian"));
        assert_eq!(p.audit_only(), ["consistency_auditor", "pacing_reviewer"]);
    }

    #[test]
    fn short_drama_defaults_include_script_writer() {
        let p = PipelineConfig::defaults_for_mode("short_drama");
        assert!(p.is_pipeline_agent("script_writer"));
        assert!(p.is_pipeline_agent("episode_planner"));
        assert!(p.mvp().iter().any(|a| a == "script_writer"));
        assert!(p.handler_for("beat_acceptor").is_some());
        assert!(!p.is_pipeline_agent("foreshadow_tracker"));
    }

    #[test]
    fn defaults_handlers_include_specialist_focus() {
        let p = PipelineConfig::defaults();
        let d = p.handler_for("dialogue_specialist").expect("dialogue handler");
        assert_eq!(d.kind, HandlerKind::SpecialistRewrite);
        assert!(d.focus.as_ref().is_some_and(|f| !f.is_empty()));
        let lit = p.handler_for("literary_editor").expect("literary handler");
        assert_eq!(lit.kind, HandlerKind::SpecialistRewrite);
        assert!(lit.focus.as_ref().is_some_and(|f| {
            f.contains("修重复") && f.contains("设定") && f.contains("倒计时")
        }));
        let order = p.order();
        let lit_i = order.iter().position(|a| a == "literary_editor");
        let pace_i = order.iter().position(|a| a == "pacing_reviewer");
        let cons_i = order.iter().position(|a| a == "consistency_auditor");
        assert!(
            lit_i.zip(cons_i).is_some_and(|(l, c)| l < c),
            "literary_editor must run before consistency_auditor"
        );
        assert!(
            pace_i.zip(cons_i).is_some_and(|(p, c)| p < c),
            "pacing_reviewer must run before consistency_auditor"
        );
        assert!(p.handler_for("writer").is_some_and(|h| h.kind == HandlerKind::Writer));
        assert!(p.handler_for("no_such_agent").is_none());
    }

    #[test]
    fn order_agents_all_have_handlers() {
        let p = PipelineConfig::defaults();
        for a in p.order() {
            assert!(
                p.handler_for(a).is_some(),
                "order agent `{a}` missing handler"
            );
        }
    }

    #[test]
    fn partial_handlers_merge_defaults() {
        let yaml = r#"
order: [writer, summarizer, dialogue_specialist]
mvp: [writer]
handlers:
  dialogue_specialist:
    kind: specialist_rewrite
    focus: "custom focus only"
"#;
        let file: PipelineFile = serde_yaml::from_str(yaml).unwrap();
        let p = PipelineConfig {
            inner: Arc::new(normalize(file)),
        };
        assert_eq!(
            p.handler_for("dialogue_specialist").unwrap().focus.as_deref(),
            Some("custom focus only")
        );
        assert_eq!(
            p.handler_for("writer").unwrap().kind,
            HandlerKind::Writer
        );
        assert_eq!(
            p.handler_for("summarizer").unwrap().kind,
            HandlerKind::Summarizer
        );
    }
}
