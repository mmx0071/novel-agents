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
    /// Free-text instructions while an audit gate is open.
    SteerInstructions { instructions: String },
}

#[derive(Debug, Clone, Default)]
struct TemplateVars {
    project: String,
    chapter: Option<u32>,
    volume: Option<u32>,
    chapters: Option<Vec<u32>>,
    instructions: Option<String>,
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

    /// chapter_next: published → continue only; blocked → revise only.
    pub fn chapter_next_options(&self, published: bool) -> Vec<UserInputOption> {
        let want = if published { "cn_continue" } else { "cn_revise" };
        self.options("chapter_next")
            .into_iter()
            .filter(|o| o.id == want)
            .collect()
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

    pub fn resolve_audit(
        &self,
        text: &str,
        project: &str,
        chapter: u32,
        queue_active: bool,
    ) -> Option<GateResolve> {
        let gate = if queue_active {
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
        let vars = TemplateVars {
            project: project.to_string(),
            ..Default::default()
        };
        Self::materialize(self.find_option("setup_confirm", text)?, &vars)
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
        "{{volume}}" => json!(vars.volume.unwrap_or(0)),
        "{{chapters}}" => json!(vars.chapters.clone().unwrap_or_default()),
        "{{instructions}}" => json!(vars.instructions.clone().unwrap_or_default()),
        _ => {
            let mut out = s.to_string();
            out = out.replace("{{project}}", &vars.project);
            if let Some(c) = vars.chapter {
                out = out.replace("{{chapter}}", &c.to_string());
            }
            if let Some(v) = vars.volume {
                out = out.replace("{{volume}}", &v.to_string());
            }
            if let Some(instr) = &vars.instructions {
                out = out.replace("{{instructions}}", instr);
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
        let r = g.resolve_audit("1", "demo", 9, false).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "steer_run");
                assert_eq!(args["choice"], "revise");
                assert_eq!(args["chapter"], 9);
            }
            _ => panic!("expected tool"),
        }
        let r = g.resolve_audit("2", "demo", 9, false).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "steer_run");
                assert_eq!(args["choice"], "accept");
            }
            _ => panic!("expected accept"),
        }
        let r = g.resolve_audit("跳过，审下一章", "demo", 9, true).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "audit_chapters");
                assert_eq!(args["action"], "next");
            }
            _ => panic!("expected queue next"),
        }
        assert!(g.is_known_token("2"));
        assert!(g.is_known_token("接受问题"));
        assert!(!g.is_known_token("写第10章"));
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
        let cont = g.chapter_next_options(true);
        assert_eq!(cont.len(), 1);
        assert_eq!(cont[0].id, "cn_continue");
        let fix = g.chapter_next_options(false);
        assert_eq!(fix.len(), 1);
        assert_eq!(fix[0].id, "cn_revise");
        let r = g.resolve_chapter_next("修正本章", "demo", 2).unwrap();
        match r {
            GateResolve::Tool { name, args } => {
                assert_eq!(name, "revise_chapter");
                assert_eq!(args["chapter"], 2);
            }
            _ => panic!("expected revise_chapter"),
        }
        let r = g.resolve_volume("2", "demo", 3).unwrap();
        assert!(matches!(r, GateResolve::SkipVolume));
    }
}
