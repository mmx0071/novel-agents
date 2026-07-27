//! Config-driven human gates (`config/gates.yaml`).
//! Prefer option **id**; aliases only for typed free-text replies.
//! Tool calls come from YAML `tool` + `args` templates (not hardcoded action→tool tables).

use anyhow::{Context, Result};
use novelx_protocol::UserInputOption;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
pub struct GateOptionSpec {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Option<Value>,
    /// Special non-tool resolves, e.g. `skip_volume`.
    #[serde(default)]
    pub resolve: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GateSpec {
    options: Vec<GateOptionSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct GatesFile {
    #[serde(default)]
    gates: HashMap<String, GateSpec>,
}

#[derive(Debug, Clone)]
pub enum GateResolve {
    Tool { name: String, args: Value },
    SkipVolume,
    ApplyMutation,
    DiscardMutation,
    /// Cascade-revise dependent artifacts after a mutation apply.
    SyncImpact,
    /// Keep source mutation; skip cascade revise for now.
    SkipImpact,
    /// Activate planned plot then continue_writing (no second mutation card).
    ActivatePlotWrite,
    /// Close the current decision card without a tool call.
    DismissGate,
    /// Free-text instructions while an audit gate is open.
    SteerInstructions { instructions: String },
}

#[derive(Debug, Clone, Default)]
struct TemplateVars {
    project: String,
    chapter: Option<u32>,
    suggest_chapter: Option<u32>,
    next_chapter: Option<u32>,
    volume: Option<u32>,
    chapters: Option<Vec<u32>>,
    instructions: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GateCatalog {
    gates: Arc<HashMap<String, GateSpec>>,
}

impl GateCatalog {
    pub fn load(config_root: &Path) -> Result<Self> {
        let path = config_root.join("gates.yaml");
        if !path.exists() {
            tracing::warn!(path = %path.display(), "gates.yaml missing; using built-in defaults");
            return Ok(Self::defaults());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let file: GatesFile = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse {}", path.display()))?;
        tracing::info!(count = file.gates.len(), "studio gates loaded");
        Ok(Self {
            gates: Arc::new(file.gates),
        })
    }

    pub fn defaults() -> Self {
        let yaml = include_str!("../../../config/gates.yaml");
        let file: GatesFile = serde_yaml::from_str(yaml).expect("embedded gates.yaml must parse");
        Self {
            gates: Arc::new(file.gates),
        }
    }

    pub fn options(&self, gate: &str) -> Vec<UserInputOption> {
        self.gates
            .get(gate)
            .map(|g| {
                g.options
                    .iter()
                    .map(|o| UserInputOption {
                        id: o.id.clone(),
                        label: o.label.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// chapter_next: published → continue; blocked → revise;
    /// published + plot still open (accept failed) → both.
    pub fn chapter_next_options(
        &self,
        published: bool,
        plot_accept_open: bool,
    ) -> Vec<UserInputOption> {
        let opts = self.options("chapter_next");
        if published && plot_accept_open {
            return opts
                .into_iter()
                .filter(|o| o.id == "cn_continue" || o.id == "cn_revise")
                .collect();
        }
        let want = if published { "cn_continue" } else { "cn_revise" };
        opts.into_iter().filter(|o| o.id == want).collect()
    }

    pub fn is_known_token(&self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        self.gates.values().any(|g| {
            g.options.iter().any(|o| {
                o.id == t || o.label == t || o.aliases.iter().any(|a| a == t)
            })
        })
    }

    /// Tokens that only make sense while a **queue / volume-audit / infra** gate is open.
    /// Used to dismiss stale approval clicks without hijacking「继续」/ chapter_next /
    /// 「局部修订」(the latter must fall through to revise when no gate is open).
    ///
    /// Bare digits (`1`/`2`/`3`) are **not** stale tokens: they are also used as display
    /// indices for other menus (draft_exists / LLM-numbered choices). While an audit gate
    /// is open, `resolve_audit` still matches them by option id.
    pub fn is_stale_audit_choice_token(&self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        // Typed「2」answering「1. … 2. 修订 …」must not become「审阅队列已结束」.
        if t.len() <= 2 && t.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        // Queue lifecycle / volume-audit / infra — not revise/accept (reusable intents).
        for name in ["audit_queue", "volume_audit", "audit_infra"] {
            let Some(g) = self.gates.get(name) else {
                continue;
            };
            for o in &g.options {
                let choice = o
                    .args
                    .as_ref()
                    .and_then(|a| a.get("choice"))
                    .and_then(|v| v.as_str());
                let is_revise = o.tool.as_deref() == Some("steer_run") && choice == Some("revise");
                // Keep audit_infra「暂不处理」(accept) as stale-dismiss; content accept is not.
                let is_content_accept = name != "audit_infra"
                    && o.tool.as_deref() == Some("steer_run")
                    && choice == Some("accept");
                if is_revise || is_content_accept {
                    continue;
                }
                if o.id == t || o.label == t || o.aliases.iter().any(|a| a == t) {
                    return true;
                }
            }
        }
        false
    }

    /// 「按审校局部修订」/ aliases — valid even after a *passed* audit (optional P1/P2 polish).
    pub fn is_audit_revise_choice(&self, text: &str) -> bool {
        self.option_matches_steer_choice(text, "revise")
    }

    /// 「接受问题」/ aliases — soft dismiss when no gate is open.
    pub fn is_audit_accept_choice(&self, text: &str) -> bool {
        self.option_matches_steer_choice(text, "accept")
    }

    fn option_matches_steer_choice(&self, text: &str, choice: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        for name in ["audit", "audit_queue"] {
            let Some(g) = self.gates.get(name) else {
                continue;
            };
            for o in &g.options {
                if o.tool.as_deref() != Some("steer_run") {
                    continue;
                }
                if o.args.as_ref().and_then(|a| a.get("choice")).and_then(|v| v.as_str())
                    != Some(choice)
                {
                    continue;
                }
                if o.id == t || o.label == t || o.aliases.iter().any(|a| a == t) {
                    return true;
                }
            }
        }
        false
    }

    fn find_option<'a>(&'a self, gate: &str, text: &str) -> Option<&'a GateOptionSpec> {
        let t = text.trim();
        let spec = self.gates.get(gate)?;
        for o in &spec.options {
            if o.id == t || o.label == t || o.aliases.iter().any(|a| a == t) {
                return Some(o);
            }
        }
        None
    }

    fn materialize(opt: &GateOptionSpec, vars: &TemplateVars) -> Option<GateResolve> {
        if let Some(resolve) = opt.resolve.as_deref() {
            return match resolve {
                "skip_volume" => Some(GateResolve::SkipVolume),
                "apply_mutation" => Some(GateResolve::ApplyMutation),
                "discard_mutation" => Some(GateResolve::DiscardMutation),
                "sync_impact" => Some(GateResolve::SyncImpact),
                "skip_impact" => Some(GateResolve::SkipImpact),
                "activate_plot_write" => Some(GateResolve::ActivatePlotWrite),
                "dismiss_gate" => Some(GateResolve::DismissGate),
                other => {
                    tracing::warn!(resolve = other, "unknown gate resolve");
                    None
                }
            };
        }
        let name = opt.tool.as_ref()?.clone();
        let args = render_args(opt.args.as_ref().unwrap_or(&Value::Object(Map::new())), vars);
        Some(GateResolve::Tool { name, args })
    }

    pub fn resolve_chapter_order(
        &self,
        text: &str,
        project: &str,
        next_chapter: u32,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            next_chapter: Some(next_chapter),
            chapter: Some(next_chapter),
            ..Default::default()
        };
        self.find_option("chapter_order", text)
            .and_then(|opt| Self::materialize(opt, &vars))
    }

    pub fn resolve_mutation_confirm(&self, text: &str) -> Option<GateResolve> {
        self.find_option("confirm_mutation", text)
            .and_then(|opt| Self::materialize(opt, &TemplateVars::default()))
    }

    pub fn mutation_confirm_options(&self) -> Vec<UserInputOption> {
        self.options("confirm_mutation")
    }

    pub fn resolve_impact_confirm(&self, text: &str) -> Option<GateResolve> {
        self.find_option("confirm_impact", text)
            .and_then(|opt| Self::materialize(opt, &TemplateVars::default()))
    }

    pub fn impact_confirm_options(&self) -> Vec<UserInputOption> {
        self.options("confirm_impact")
    }

    pub fn chapter_order_options(&self, next_chapter: u32) -> Vec<UserInputOption> {
        self.options("chapter_order")
            .into_iter()
            .map(|mut o| {
                o.label = o.label.replace("{{next_chapter}}", &next_chapter.to_string());
                o
            })
            .collect()
    }

    pub fn resolve_audit(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
        queue_active: bool,
        infra: bool,
    ) -> Option<GateResolve> {
        let gate = if infra {
            "audit_infra"
        } else if queue_active {
            "audit_queue"
        } else {
            "audit"
        };
        let vars = TemplateVars {
            project: project.to_string(),
            chapter: Some(chapter),
            ..Default::default()
        };
        if let Some(opt) = self.find_option(gate, text) {
            return Self::materialize(opt, &vars);
        }

        // Infra: only explicit retry/skip — never free-text → local revise.
        if infra {
            return None;
        }

        // Free-text while gate open → revise with instructions (not a chapter op).
        let t = text.trim();
        if t.chars().count() < 4 || self.is_known_token(t) {
            return None;
        }
        if t.contains('章')
            && (t.contains("审校")
                || t.contains("修正")
                || t.contains("扩写")
                || t.contains("重写")
                || t.contains("续写")
                || t.contains("写第"))
        {
            return None;
        }
        Some(GateResolve::SteerInstructions {
            instructions: t.to_string(),
        })
    }

    pub fn resolve_volume(
        &self,
        text: &str,
        project: &str,
        volume: u32,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            volume: Some(volume),
            ..Default::default()
        };
        Self::materialize(self.find_option("volume_sync", text)?, &vars)
    }

    pub fn resolve_volume_audit(
        &self,
        text: &str,
        project: &str,
        chapters: &[u32],
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            chapters: Some(chapters.to_vec()),
            ..Default::default()
        };
        Self::materialize(self.find_option("volume_audit", text)?, &vars)
    }

    pub fn resolve_setup(&self, text: &str, project: &str) -> Option<GateResolve> {
        self.resolve_setup_progress(text, project, "setup_confirm")
    }

    pub fn resolve_setup_progress(
        &self,
        text: &str,
        project: &str,
        gate: &str,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            ..Default::default()
        };
        Self::materialize(self.find_option(gate, text)?, &vars)
    }

    pub fn resolve_setup_progress_visible(
        &self,
        text: &str,
        project: &str,
        gate: &str,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_setup_progress(text, project, gate) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.options(gate);
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_setup_progress(&id, project, gate)
    }

    /// Volume handoff: design next arc, or design+activate next plot card.
    pub fn resolve_volume_handoff(
        &self,
        text: &str,
        project: &str,
        phase: &str,
        volume: u32,
        title: &str,
    ) -> Option<GateResolve> {
        let gate = match phase {
            "awaiting_next_arc" => "volume_handoff_arc",
            "awaiting_next_plot" => "volume_handoff_plot",
            _ => return None,
        };
        let vars = TemplateVars {
            project: project.to_string(),
            volume: Some(volume),
            title: Some(title.to_string()),
            ..Default::default()
        };
        Self::materialize(self.find_option(gate, text)?, &vars)
    }

    pub fn resolve_volume_handoff_visible(
        &self,
        text: &str,
        project: &str,
        phase: &str,
        volume: u32,
        title: &str,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_volume_handoff(text, project, phase, volume, title) {
            return Some(r);
        }
        let gate = match phase {
            "awaiting_next_arc" => "volume_handoff_arc",
            "awaiting_next_plot" => "volume_handoff_plot",
            _ => return None,
        };
        let idx = parse_one_based_index(text)?;
        let opts = self.options(gate);
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_volume_handoff(&id, project, phase, volume, title)
    }

    pub fn resolve_chapter_next(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            chapter: Some(chapter),
            ..Default::default()
        };
        Self::materialize(self.find_option("chapter_next", text)?, &vars)
    }

    pub fn resolve_draft_exists(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
        suggest_chapter: u32,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            chapter: Some(chapter),
            suggest_chapter: Some(suggest_chapter),
            ..Default::default()
        };
        Self::materialize(self.find_option("draft_exists", text)?, &vars)
    }

    pub fn planned_plot_options(&self, title: &str) -> Vec<UserInputOption> {
        self.options("planned_plot")
            .into_iter()
            .map(|mut o| {
                o.label = o.label.replace("{{title}}", title);
                o
            })
            .collect()
    }

    pub fn resolve_planned_plot(&self, text: &str, project: &str, title: &str) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            title: Some(title.to_string()),
            ..Default::default()
        };
        Self::materialize(self.find_option("planned_plot", text)?, &vars)
    }

    pub fn resolve_planned_plot_visible(
        &self,
        text: &str,
        project: &str,
        title: &str,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_planned_plot(text, project, title) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.planned_plot_options(title);
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_planned_plot(&id, project, title)
    }

    pub fn need_plot_options(&self) -> Vec<UserInputOption> {
        self.options("need_plot")
    }

    pub fn resolve_need_plot(
        &self,
        text: &str,
        project: &str,
        volume: u32,
        title: &str,
    ) -> Option<GateResolve> {
        let vars = TemplateVars {
            project: project.to_string(),
            volume: Some(volume),
            title: Some(title.to_string()),
            ..Default::default()
        };
        Self::materialize(self.find_option("need_plot", text)?, &vars)
    }

    pub fn resolve_need_plot_visible(
        &self,
        text: &str,
        project: &str,
        volume: u32,
        title: &str,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_need_plot(text, project, volume, title) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.need_plot_options();
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_need_plot(&id, project, volume, title)
    }

    pub fn resolve_draft_exists_visible(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
        suggest_chapter: u32,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_draft_exists(text, project, chapter, suggest_chapter) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.options("draft_exists");
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_draft_exists(&id, project, chapter, suggest_chapter)
    }

    /// Map typed「1」「2」… to the Nth *visible* option (ApprovalOptions shows 1-based index,
    /// but ids may be `cn_revise` / `sc_approve` / `va_deep`, not bare digits).
    pub fn resolve_chapter_next_visible(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
        published: bool,
        plot_accept_open: bool,
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_chapter_next(text, project, chapter) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.chapter_next_options(published, plot_accept_open);
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_chapter_next(&id, project, chapter)
    }

    pub fn resolve_setup_visible(&self, text: &str, project: &str) -> Option<GateResolve> {
        self.resolve_setup_progress_visible(text, project, "setup_confirm")
    }

    pub fn resolve_volume_audit_visible(
        &self,
        text: &str,
        project: &str,
        chapters: &[u32],
    ) -> Option<GateResolve> {
        if let Some(r) = self.resolve_volume_audit(text, project, chapters) {
            return Some(r);
        }
        let idx = parse_one_based_index(text)?;
        let opts = self.options("volume_audit");
        let id = opts.get(idx.checked_sub(1)?)?.id.clone();
        self.resolve_volume_audit(&id, project, chapters)
    }
}

/// Bare「1」/「2」from the approval hint「也可输入序号」.
fn parse_one_based_index(text: &str) -> Option<usize> {
    let t = text.trim();
    if t.is_empty() || t.len() > 2 || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: usize = t.parse().ok()?;
    (n >= 1).then_some(n)
}

fn render_args(template: &Value, vars: &TemplateVars) -> Value {
    match template {
        Value::String(s) => render_string_value(s, vars),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| render_args(v, vars)).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), render_args(v, vars));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn render_string_value(s: &str, vars: &TemplateVars) -> Value {
    match s {
        "{{project}}" => json!(vars.project),
        "{{chapter}}" => json!(vars.chapter.unwrap_or(0)),
        "{{suggest_chapter}}" => json!(vars.suggest_chapter.unwrap_or(0)),
        "{{next_chapter}}" => json!(vars.next_chapter.unwrap_or(0)),
        "{{volume}}" => json!(vars.volume.unwrap_or(0)),
        "{{chapters}}" => json!(vars.chapters.clone().unwrap_or_default()),
        "{{instructions}}" => json!(vars.instructions.clone().unwrap_or_default()),
        "{{title}}" => json!(vars.title.clone().unwrap_or_default()),
        _ => {
            let mut out = s.to_string();
            out = out.replace("{{project}}", &vars.project);
            if let Some(c) = vars.chapter {
                out = out.replace("{{chapter}}", &c.to_string());
            }
            if let Some(c) = vars.suggest_chapter {
                out = out.replace("{{suggest_chapter}}", &c.to_string());
            }
            if let Some(c) = vars.next_chapter {
                out = out.replace("{{next_chapter}}", &c.to_string());
            }
            if let Some(v) = vars.volume {
                out = out.replace("{{volume}}", &v.to_string());
            }
            if let Some(instr) = &vars.instructions {
                out = out.replace("{{instructions}}", instr);
            }
            if let Some(title) = &vars.title {
                out = out.replace("{{title}}", title);
            }
            json!(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_by_id_and_alias() {
        let g = GateCatalog::defaults();
        let r = g.resolve_audit("1", "demo", 9, false, false).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "steer_run");
                assert_eq!(args["choice"], "revise");
                assert_eq!(args["chapter"], 9);
            }
            _ => panic!("expected tool"),
        }
        let r = g.resolve_audit("2", "demo", 9, false, false).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "steer_run");
                assert_eq!(args["choice"], "accept");
            }
            _ => panic!("expected accept"),
        }
        let r = g.resolve_audit("跳过，审下一章", "demo", 9, true, false).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "audit_chapters");
                assert_eq!(args["action"], "next");
            }
            _ => panic!("expected queue next"),
        }
        let r = g.resolve_audit("重新审计", "demo", 13, false, true).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "audit_chapter");
                assert_eq!(args["chapter"], 13);
            }
            _ => panic!("expected infra retry"),
        }
        assert!(g.resolve_audit("随便改改节奏", "demo", 13, false, true).is_none());
        assert!(g.is_known_token("2"));
        assert!(g.is_known_token("接受问题"));
        assert!(!g.is_known_token("写第10章"));
        // Revise/accept are reusable intents — must NOT become「审阅队列已结束」.
        assert!(!g.is_stale_audit_choice_token("接受问题"));
        assert!(!g.is_stale_audit_choice_token("局部修订"));
        assert!(!g.is_stale_audit_choice_token("按审校局部修订"));
        assert!(g.is_audit_revise_choice("局部修订"));
        assert!(g.is_audit_accept_choice("接受问题"));
        // Queue lifecycle / infra still stale when no gate is open.
        assert!(g.is_stale_audit_choice_token("跳过，审下一章"));
        assert!(g.is_stale_audit_choice_token("结束审阅队列"));
        assert!(g.is_stale_audit_choice_token("重试审计"));
        // Bare digits are NOT stale — they collide with other numbered menus.
        assert!(!g.is_stale_audit_choice_token("1"));
        assert!(!g.is_stale_audit_choice_token("2"));
        assert!(!g.is_stale_audit_choice_token("继续"));
        assert!(!g.is_stale_audit_choice_token("继续创作"));
        assert!(!g.is_stale_audit_choice_token("写第10章"));
        let r = g
            .resolve_volume_audit("va_deep", "demo", &[1, 3, 8])
            .unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "audit_chapters");
                assert_eq!(args["action"], "start");
                assert_eq!(args["chapters"], json!([1, 3, 8]));
            }
            _ => panic!("expected deep audit tool"),
        }
        // Must not steal audit_queue option id "1".
        assert!(g.resolve_volume_audit("1", "demo", &[1]).is_none());
        let r = g.resolve_setup("sc_approve", "demo").unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "confirm_setup");
                assert_eq!(args["action"], "approve");
            }
            _ => panic!("expected confirm_setup"),
        }
        let r = g.resolve_setup("修改再生成", "demo").unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "confirm_setup");
                assert_eq!(args["action"], "revise");
            }
            _ => panic!("expected revise"),
        }
        let r = g.resolve_chapter_next("cn_continue", "demo", 2).unwrap();
        match r {
            GateResolve::Tool { name, .. } => assert_eq!(name, "continue_writing"),
            _ => panic!("expected continue_writing"),
        }
        let r = g
            .resolve_draft_exists_visible("de_next", "demo", 11, 12)
            .unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "continue_writing");
                assert_eq!(args["chapter"], 12);
            }
            _ => panic!("expected continue_writing ch12"),
        }
        let r = g.resolve_draft_exists("审校本章", "demo", 11, 12).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "audit_chapter");
                assert_eq!(args["chapter"], 11);
            }
            _ => panic!("expected audit_chapter"),
        }
        let opts = g.planned_plot_options("样例卡");
        assert!(opts.iter().any(|o| o.label.contains("样例卡")));
        assert!(matches!(
            g.resolve_planned_plot("激活并写章", "demo", "样例卡"),
            Some(GateResolve::ActivatePlotWrite)
        ));
        assert!(matches!(
            g.resolve_planned_plot("暂不写作", "demo", "样例卡"),
            Some(GateResolve::DismissGate)
        ));
        assert!(matches!(
            g.resolve_need_plot("设计并激活剧情卡", "demo", 2, "下一段"),
            Some(GateResolve::Tool { name, .. }) if name == "design_plot"
        ));
        let cont = g.chapter_next_options(true, false);
        assert_eq!(cont.len(), 1);
        assert_eq!(cont[0].id, "cn_continue");
        let fix = g.chapter_next_options(false, false);
        assert_eq!(fix.len(), 1);
        assert_eq!(fix[0].id, "cn_revise");
        let open = g.chapter_next_options(true, true);
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].id, "cn_continue");
        assert_eq!(open[1].id, "cn_revise");
        let r = g.resolve_chapter_next("修正本章", "demo", 2).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "revise_chapter");
                assert_eq!(args["chapter"], 2);
            }
            _ => panic!("expected revise_chapter"),
        }
        // UI shows「1. 修正本章」even though id is cn_revise — typed「1」must work.
        let r = g
            .resolve_chapter_next_visible("1", "demo", 10, false, false)
            .unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "revise_chapter");
                assert_eq!(args["chapter"], 10);
            }
            _ => panic!("expected revise via display index"),
        }
        let r = g
            .resolve_chapter_next_visible("1", "demo", 3, true, false)
            .unwrap();
        match r {
            GateResolve::Tool { name, .. } => assert_eq!(name, "continue_writing"),
            _ => panic!("expected continue via display index"),
        }
        let r = g
            .resolve_chapter_next_visible("2", "demo", 3, true, true)
            .unwrap();
        match r {
            GateResolve::Tool { name, .. } => assert_eq!(name, "revise_chapter"),
            _ => panic!("expected revise as second option when plot open"),
        }
        let r = g.resolve_setup_visible("1", "demo").unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "confirm_setup");
                assert_eq!(args["action"], "approve");
            }
            _ => panic!("expected setup approve via index"),
        }
        let r = g
            .resolve_setup_progress_visible("生成总纲", "demo", "setup_need_master")
            .unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "design_master_outline");
                assert_eq!(args["project"], "demo");
            }
            _ => panic!("expected design_master_outline"),
        }
        assert!(matches!(
            g.resolve_setup_progress_visible("暂不处理", "demo", "setup_need_master")
                .unwrap(),
            GateResolve::DismissGate
        ));
        let r = g.resolve_volume("2", "demo", 3).unwrap();
        assert!(matches!(r, GateResolve::SkipVolume));
        let r = g.resolve_chapter_order("写第{{next_chapter}}章".replace("{{next_chapter}}", "4").as_str(), "demo", 4)
            .or_else(|| g.resolve_chapter_order("写下一章", "demo", 4))
            .unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "continue_writing");
                assert_eq!(args["chapter"], 4);
            }
            _ => panic!("expected continue_writing next chapter"),
        }
        assert!(matches!(
            g.resolve_mutation_confirm("应用修改").unwrap(),
            GateResolve::ApplyMutation
        ));
        assert!(matches!(
            g.resolve_mutation_confirm("放弃").unwrap(),
            GateResolve::DiscardMutation
        ));
        // Setup-confirm mis-click while a mutation preview is open.
        assert!(matches!(
            g.resolve_mutation_confirm("sc_approve").unwrap(),
            GateResolve::ApplyMutation
        ));
        assert!(matches!(
            g.resolve_mutation_confirm("确认定稿").unwrap(),
            GateResolve::ApplyMutation
        ));
        assert!(matches!(
            g.resolve_impact_confirm("自动同步修正").unwrap(),
            GateResolve::SyncImpact
        ));
        assert!(matches!(
            g.resolve_impact_confirm("暂不同步").unwrap(),
            GateResolve::SkipImpact
        ));
    }
}
