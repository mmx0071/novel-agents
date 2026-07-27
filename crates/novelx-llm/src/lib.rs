//! OpenAI-compatible LLM client (DeepSeek etc.).

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("missing api key")]
    MissingKey,
    #[error("http: {0}")]
    Http(String),
    #[error("parse: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub default_model: String,
    pub base_url: String,
    pub api_key_env: String,
    /// task name → model id
    pub tasks: HashMap<String, String>,
    /// agent id → task name
    pub agents: HashMap<String, String>,
    /// task name → max_tokens (from llm.yaml)
    #[serde(default)]
    pub task_max_tokens: HashMap<String, u32>,
    #[serde(default = "default_max_tokens")]
    pub default_max_tokens: u32,
    /// Extra attempts after the first failure (0 = no retry).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
    #[serde(default = "default_retry_max_delay_ms")]
    pub retry_max_delay_ms: u64,
}

fn default_max_tokens() -> u32 {
    131072
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_base_delay_ms() -> u64 {
    800
}

fn default_retry_max_delay_ms() -> u64 {
    10_000
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_model: "deepseek-v4-flash".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            tasks: HashMap::new(),
            agents: HashMap::new(),
            task_max_tokens: HashMap::new(),
            default_max_tokens: default_max_tokens(),
            max_retries: default_max_retries(),
            retry_base_delay_ms: default_retry_base_delay_ms(),
            retry_max_delay_ms: default_retry_max_delay_ms(),
        }
    }
}

pub fn load_llm_config(path: &Path) -> Result<LlmConfig> {
    if !path.exists() {
        return Ok(LlmConfig::default());
    }
    let text = std::fs::read_to_string(path)?;
    let v: serde_yaml::Value = serde_yaml::from_str(&text)?;
    let mut cfg = LlmConfig::default();

    if let Some(model) = v.get("default_model").and_then(|x| x.as_str()) {
        cfg.default_model = model.to_string();
    }
    if let Some(url) = v.get("base_url").and_then(|x| x.as_str()) {
        cfg.base_url = url.to_string();
    }
    if let Some(env) = v.get("api_key_env").and_then(|x| x.as_str()) {
        cfg.api_key_env = env.to_string();
    }

    // Prefer an explicit default provider, else the first provider that has a key/base_url.
    // Do NOT walk all providers — later unused entries (openai/anthropic) would overwrite
    // api_key_env and cause false "missing API key" placeholders.
    let preferred = v
        .get("default_provider")
        .and_then(|x| x.as_str())
        .unwrap_or("deepseek");
    if let Some(providers) = v.get("providers").and_then(|x| x.as_mapping()) {
        let pick = providers
            .get(serde_yaml::Value::String(preferred.into()))
            .or_else(|| {
                providers.values().find(|prov| {
                    prov.get("base_url")
                        .and_then(|x| x.as_str())
                        .is_some_and(|s| !s.is_empty())
                })
            });
        if let Some(prov) = pick {
            if let Some(url) = prov.get("base_url").and_then(|x| x.as_str()) {
                // DeepSeek OpenAI-compatible path is /v1/chat/completions
                let url = if url.contains("/v1") {
                    url.to_string()
                } else {
                    format!("{}/v1", url.trim_end_matches('/'))
                };
                cfg.base_url = url;
            }
            if let Some(env) = prov.get("api_key_env").and_then(|x| x.as_str()) {
                cfg.api_key_env = env.to_string();
            }
            if let Some(m) = prov.get("default_model").and_then(|x| x.as_str()) {
                cfg.default_model = m.to_string();
            }
        }
    }

    // Prefer first task model as default when set (e.g. deepseek-v4-pro)
    if let Some(tasks) = v.get("tasks").and_then(|x| x.as_mapping()) {
        for key in ["creative", "planning", "editing"] {
            if let Some(m) = tasks
                .get(serde_yaml::Value::String(key.into()))
                .and_then(|t| t.get("model"))
                .and_then(|x| x.as_str())
            {
                cfg.default_model = m.to_string();
                break;
            }
        }
    }

    if let Some(tasks) = v.get("tasks").and_then(|x| x.as_mapping()) {
        for (k, val) in tasks {
            let Some(ks) = k.as_str() else { continue };
            if let Some(vs) = val.as_str() {
                cfg.tasks.insert(ks.to_string(), vs.to_string());
            } else if let Some(m) = val.get("model").and_then(|x| x.as_str()) {
                cfg.tasks.insert(ks.to_string(), m.to_string());
            }
            if let Some(n) = val.get("max_tokens").and_then(|x| x.as_u64()) {
                cfg.task_max_tokens.insert(ks.to_string(), n as u32);
            }
        }
    }

    if let Some(agents) = v.get("agents").and_then(|x| x.as_mapping()) {
        for (k, val) in agents {
            let Some(ks) = k.as_str() else { continue };
            if let Some(vs) = val.as_str() {
                cfg.agents.insert(ks.to_string(), vs.to_string());
            } else if let Some(m) = val
                .get("task")
                .or_else(|| val.get("model"))
                .and_then(|x| x.as_str())
            {
                cfg.agents.insert(ks.to_string(), m.to_string());
            }
        }
    }

    // Dev profile: force cheapest/fastest model for every task (testing efficiency).
    let profile = std::env::var("NOVELX_LLM_PROFILE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            v.get("profile")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "dev".into());
    let profile = profile.to_ascii_lowercase();
    if profile == "dev" || profile == "development" {
        let force = v
            .get("dev_model")
            .and_then(|x| x.as_str())
            .unwrap_or("deepseek-v4-flash")
            .to_string();
        cfg.default_model = force.clone();
        for model in cfg.tasks.values_mut() {
            *model = force.clone();
        }
        tracing::info!(%force, "LLM profile=dev: all tasks use cheapest/fastest model");
    } else {
        tracing::info!(%profile, model = %cfg.default_model, "LLM profile loaded");
    }

    if let Some(retry) = v.get("retry") {
        if let Some(n) = retry.get("max_retries").and_then(|x| x.as_u64()) {
            cfg.max_retries = n.min(8) as u32;
        }
        if let Some(n) = retry.get("base_delay_ms").and_then(|x| x.as_u64()) {
            cfg.retry_base_delay_ms = n.max(100);
        }
        if let Some(n) = retry.get("max_delay_ms").and_then(|x| x.as_u64()) {
            cfg.retry_max_delay_ms = n.max(cfg.retry_base_delay_ms);
        }
    }
    if let Ok(n) = std::env::var("NOVELX_LLM_MAX_RETRIES") {
        if let Ok(n) = n.parse::<u32>() {
            cfg.max_retries = n.min(8);
        }
    }

    Ok(cfg)
}

/// Ordered task keys shown / edited in the Web form.
pub const LLM_FORM_TASKS: &[&str] = &[
    "planning",
    "creative",
    "editing",
    "patch",
    "analysis",
    "naming",
    "studio",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRetryForm {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderForm {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub api_key_env: String,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_suffix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmTaskForm {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub model: String,
    pub max_tokens: u32,
}

/// Structured LLM settings for the Web config form (no plaintext API keys).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFormSettings {
    pub profile: String,
    pub dev_model: String,
    pub default_provider: String,
    pub providers: Vec<LlmProviderForm>,
    pub tasks: Vec<LlmTaskForm>,
    pub retry: LlmRetryForm,
}

/// PUT body: form fields + optional write-only API key for the active provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFormPut {
    pub profile: String,
    pub dev_model: String,
    pub default_provider: String,
    pub providers: Vec<LlmProviderForm>,
    pub tasks: Vec<LlmTaskForm>,
    pub retry: LlmRetryForm,
    /// Write-only. Empty / omitted = keep existing key.
    #[serde(default)]
    pub api_key: Option<String>,
}

fn yaml_str(v: &serde_yaml::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// Load form-oriented settings from `llm.yaml` (file values, not profile-forced runtime).
pub fn load_llm_form_settings(path: &Path) -> Result<LlmFormSettings> {
    let defaults = LlmConfig::default();
    let mut settings = LlmFormSettings {
        profile: "dev".into(),
        dev_model: "deepseek-v4-flash".into(),
        default_provider: "deepseek".into(),
        providers: vec![LlmProviderForm {
            name: "deepseek".into(),
            base_url: Some("https://api.deepseek.com".into()),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            has_api_key: env_has_api_key("DEEPSEEK_API_KEY"),
            api_key_suffix: env_api_key_suffix("DEEPSEEK_API_KEY"),
        }],
        tasks: LLM_FORM_TASKS
            .iter()
            .map(|name| LlmTaskForm {
                name: (*name).into(),
                description: String::new(),
                model: defaults.default_model.clone(),
                max_tokens: defaults.default_max_tokens,
            })
            .collect(),
        retry: LlmRetryForm {
            max_retries: defaults.max_retries,
            base_delay_ms: defaults.retry_base_delay_ms,
            max_delay_ms: defaults.retry_max_delay_ms,
        },
    };

    if !path.exists() {
        return Ok(settings);
    }
    let text = std::fs::read_to_string(path)?;
    let v: serde_yaml::Value = serde_yaml::from_str(&text)?;

    if let Some(p) = yaml_str(&v, "profile") {
        settings.profile = p;
    }
    if let Some(m) = yaml_str(&v, "dev_model") {
        settings.dev_model = m;
    }
    if let Some(p) = yaml_str(&v, "default_provider") {
        settings.default_provider = p;
    } else {
        settings.default_provider = "deepseek".into();
    }

    if let Some(retry) = v.get("retry") {
        if let Some(n) = retry.get("max_retries").and_then(|x| x.as_u64()) {
            settings.retry.max_retries = n.min(8) as u32;
        }
        if let Some(n) = retry.get("base_delay_ms").and_then(|x| x.as_u64()) {
            settings.retry.base_delay_ms = n.max(100);
        }
        if let Some(n) = retry.get("max_delay_ms").and_then(|x| x.as_u64()) {
            settings.retry.max_delay_ms = n.max(settings.retry.base_delay_ms);
        }
    }

    if let Some(providers) = v.get("providers").and_then(|x| x.as_mapping()) {
        let mut list = Vec::new();
        for (k, prov) in providers {
            let Some(name) = k.as_str() else { continue };
            let base_url = prov
                .get("base_url")
                .and_then(|x| {
                    if x.is_null() {
                        None
                    } else {
                        x.as_str().map(|s| s.to_string())
                    }
                });
            let api_key_env = prov
                .get("api_key_env")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let has_api_key = !api_key_env.is_empty() && env_has_api_key(&api_key_env);
            let api_key_suffix = if has_api_key {
                env_api_key_suffix(&api_key_env)
            } else {
                None
            };
            list.push(LlmProviderForm {
                name: name.to_string(),
                base_url,
                api_key_env,
                has_api_key,
                api_key_suffix,
            });
        }
        if !list.is_empty() {
            settings.providers = list;
        }
    }

    if let Some(tasks) = v.get("tasks").and_then(|x| x.as_mapping()) {
        let mut by_name: HashMap<String, LlmTaskForm> = HashMap::new();
        for (k, val) in tasks {
            let Some(name) = k.as_str() else { continue };
            let model = if let Some(m) = val.as_str() {
                m.to_string()
            } else {
                val.get("model")
                    .and_then(|x| x.as_str())
                    .unwrap_or(&defaults.default_model)
                    .to_string()
            };
            let max_tokens = val
                .get("max_tokens")
                .and_then(|x| x.as_u64())
                .map(|n| n as u32)
                .unwrap_or(defaults.default_max_tokens);
            let description = val
                .get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            by_name.insert(
                name.to_string(),
                LlmTaskForm {
                    name: name.to_string(),
                    description,
                    model,
                    max_tokens,
                },
            );
        }
        let mut ordered = Vec::new();
        for name in LLM_FORM_TASKS {
            if let Some(t) = by_name.remove(*name) {
                ordered.push(t);
            }
        }
        for (_, t) in by_name {
            ordered.push(t);
        }
        if !ordered.is_empty() {
            settings.tasks = ordered;
        }
    }

    Ok(settings)
}

/// Validate form PUT payload. Never includes API key material in error messages.
pub fn validate_llm_form_put(put: &LlmFormPut) -> Result<(), String> {
    let profile = put.profile.trim().to_ascii_lowercase();
    if profile != "dev" && profile != "prod" && profile != "development" && profile != "production" {
        return Err("profile 须为 dev 或 prod".into());
    }
    if put.dev_model.trim().is_empty() {
        return Err("dev_model 不能为空".into());
    }
    if put.default_provider.trim().is_empty() {
        return Err("default_provider 不能为空".into());
    }
    if put.providers.is_empty() {
        return Err("至少需要一个 provider".into());
    }
    let active = put
        .providers
        .iter()
        .find(|p| p.name == put.default_provider)
        .ok_or_else(|| format!("default_provider「{}」不在 providers 列表中", put.default_provider))?;
    let base = active.base_url.as_deref().unwrap_or("").trim();
    if base.is_empty() {
        return Err("当前 provider 的 base_url 不能为空".into());
    }
    if active.api_key_env.trim().is_empty() {
        return Err("当前 provider 的 api_key_env 不能为空".into());
    }
    if put.tasks.is_empty() {
        return Err("至少需要一个 task".into());
    }
    for t in &put.tasks {
        if t.name.trim().is_empty() {
            return Err("task name 不能为空".into());
        }
        if t.model.trim().is_empty() {
            return Err(format!("任务「{}」的 model 不能为空", t.name));
        }
        if !(256..=384_000).contains(&t.max_tokens) {
            return Err(format!(
                "任务「{}」的 max_tokens 须在 256..=384000",
                t.name
            ));
        }
    }
    if put.retry.max_retries > 8 {
        return Err("max_retries 上限为 8".into());
    }
    if put.retry.base_delay_ms < 100 {
        return Err("base_delay_ms 至少 100".into());
    }
    if put.retry.max_delay_ms < put.retry.base_delay_ms {
        return Err("max_delay_ms 不能小于 base_delay_ms".into());
    }
    if let Some(key) = put.api_key.as_deref() {
        let key = key.trim();
        if !key.is_empty() && key.len() < 8 {
            return Err("API Key 过短".into());
        }
    }
    Ok(())
}

/// Patch `llm.yaml` Value with form fields; preserves agents and per-task extras (e.g. temperature).
pub fn apply_llm_form_to_yaml(
    existing: &str,
    put: &LlmFormPut,
) -> Result<String, String> {
    let mut root: serde_yaml::Value = if existing.trim().is_empty() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(existing).map_err(|e| format!("解析现有 llm.yaml 失败：{e}"))?
    };
    if !root.is_mapping() {
        root = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }
    let map = root.as_mapping_mut().expect("mapping");

    map.insert(
        serde_yaml::Value::String("profile".into()),
        serde_yaml::Value::String(put.profile.trim().to_ascii_lowercase()),
    );
    map.insert(
        serde_yaml::Value::String("dev_model".into()),
        serde_yaml::Value::String(put.dev_model.trim().to_string()),
    );
    map.insert(
        serde_yaml::Value::String("default_provider".into()),
        serde_yaml::Value::String(put.default_provider.trim().to_string()),
    );

    let mut retry_map = serde_yaml::Mapping::new();
    retry_map.insert(
        serde_yaml::Value::String("max_retries".into()),
        serde_yaml::Value::Number(put.retry.max_retries.into()),
    );
    retry_map.insert(
        serde_yaml::Value::String("base_delay_ms".into()),
        serde_yaml::Value::Number(put.retry.base_delay_ms.into()),
    );
    retry_map.insert(
        serde_yaml::Value::String("max_delay_ms".into()),
        serde_yaml::Value::Number(put.retry.max_delay_ms.into()),
    );
    map.insert(
        serde_yaml::Value::String("retry".into()),
        serde_yaml::Value::Mapping(retry_map),
    );

    let mut providers = match map
        .get(&serde_yaml::Value::String("providers".into()))
        .and_then(|x| x.as_mapping())
        .cloned()
    {
        Some(m) => m,
        None => serde_yaml::Mapping::new(),
    };
    for p in &put.providers {
        let key = serde_yaml::Value::String(p.name.clone());
        let mut prov = providers
            .get(&key)
            .and_then(|x| x.as_mapping())
            .cloned()
            .unwrap_or_default();
        match &p.base_url {
            Some(url) if !url.trim().is_empty() => {
                prov.insert(
                    serde_yaml::Value::String("base_url".into()),
                    serde_yaml::Value::String(url.trim().to_string()),
                );
            }
            _ => {
                prov.insert(
                    serde_yaml::Value::String("base_url".into()),
                    serde_yaml::Value::Null,
                );
            }
        }
        prov.insert(
            serde_yaml::Value::String("api_key_env".into()),
            serde_yaml::Value::String(p.api_key_env.trim().to_string()),
        );
        providers.insert(key, serde_yaml::Value::Mapping(prov));
    }
    map.insert(
        serde_yaml::Value::String("providers".into()),
        serde_yaml::Value::Mapping(providers),
    );

    let mut tasks = match map
        .get(&serde_yaml::Value::String("tasks".into()))
        .and_then(|x| x.as_mapping())
        .cloned()
    {
        Some(m) => m,
        None => serde_yaml::Mapping::new(),
    };
    for t in &put.tasks {
        let key = serde_yaml::Value::String(t.name.clone());
        let mut task = tasks
            .get(&key)
            .and_then(|x| x.as_mapping())
            .cloned()
            .unwrap_or_default();
        if !t.description.trim().is_empty() {
            task.insert(
                serde_yaml::Value::String("description".into()),
                serde_yaml::Value::String(t.description.trim().to_string()),
            );
        }
        task.insert(
            serde_yaml::Value::String("model".into()),
            serde_yaml::Value::String(t.model.trim().to_string()),
        );
        task.insert(
            serde_yaml::Value::String("max_tokens".into()),
            serde_yaml::Value::Number(t.max_tokens.into()),
        );
        // Keep provider alignment with default when absent.
        if !task.contains_key(serde_yaml::Value::String("provider".into())) {
            task.insert(
                serde_yaml::Value::String("provider".into()),
                serde_yaml::Value::String(put.default_provider.trim().to_string()),
            );
        }
        tasks.insert(key, serde_yaml::Value::Mapping(task));
    }
    map.insert(
        serde_yaml::Value::String("tasks".into()),
        serde_yaml::Value::Mapping(tasks),
    );

    serde_yaml::to_string(&root).map_err(|e| format!("序列化 llm.yaml 失败：{e}"))
}

/// Upsert `KEY=value` in a dotenv file; does not log the value.
pub fn upsert_dotenv_key(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.contains('=') || key.contains('\n') {
        return Err("非法环境变量名".into());
    }
    let value = value.trim();
    if value.is_empty() {
        return Err("API Key 不能为空".into());
    }
    if value.contains('\n') || value.contains('\r') {
        return Err("API Key 含非法换行".into());
    }
    let escaped = if value.bytes().any(|b| b.is_ascii_whitespace() || b == b'"' || b == b'#') {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    };
    let line = format!("{key}={escaped}");
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(path)
            .map_err(|e| format!("读取 .env 失败：{e}"))?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };
    let prefix = format!("{key}=");
    let mut found = false;
    for row in &mut lines {
        let trimmed = row.trim_start();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with(&prefix) || trimmed.split('=').next() == Some(key) {
            *row = line.clone();
            found = true;
            break;
        }
    }
    if !found {
        if lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(String::new());
        }
        lines.push(line);
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    std::fs::write(path, out).map_err(|e| format!("写入 .env 失败：{e}"))?;
    // Process-local so reload sees the new key without restart.
    unsafe { std::env::set_var(key, value) };
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    /// DeepSeek thinking mode: must be echoed back after assistant tool_calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionResult {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

fn resolve_api_key_from_env(api_key_env: &str) -> Option<String> {
    std::env::var(api_key_env)
        .ok()
        .filter(|s| !s.is_empty() && !s.contains("your-") && s != "sk-your-deepseek-key")
}

/// Last 4 chars for UI confirmation; never expose the full key.
pub fn api_key_suffix(key: &str) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let n = chars.len().min(4);
    Some(chars[chars.len() - n..].iter().collect())
}

/// Whether an env var currently holds a usable API key (no plaintext returned).
pub fn env_has_api_key(api_key_env: &str) -> bool {
    resolve_api_key_from_env(api_key_env).is_some()
}

pub fn env_api_key_suffix(api_key_env: &str) -> Option<String> {
    resolve_api_key_from_env(api_key_env).as_deref().and_then(api_key_suffix)
}

struct LlmInner {
    config: LlmConfig,
    api_key: Option<String>,
}

#[derive(Clone)]
pub struct LlmClient {
    inner: Arc<RwLock<LlmInner>>,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let api_key = resolve_api_key_from_env(&config.api_key_env);
        if api_key.is_none() {
            tracing::warn!(
                env = %config.api_key_env,
                "LLM API key missing; responses will be placeholders"
            );
        } else {
            tracing::info!(
                env = %config.api_key_env,
                model = %config.default_model,
                base = %config.base_url,
                tasks = config.tasks.len(),
                "LLM client ready"
            );
        }
        let http = reqwest::Client::builder()
            // Long creative chapters need headroom; stream also has its own deadline.
            .timeout(std::time::Duration::from_secs(240))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(2)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: Arc::new(RwLock::new(LlmInner { config, api_key })),
            http,
        }
    }

    /// Hot-reload config; re-reads API key from the process environment.
    pub fn reload(&self, config: LlmConfig) {
        let api_key = resolve_api_key_from_env(&config.api_key_env);
        if api_key.is_none() {
            tracing::warn!(
                env = %config.api_key_env,
                "LLM API key missing after reload; responses will be placeholders"
            );
        } else {
            tracing::info!(
                env = %config.api_key_env,
                model = %config.default_model,
                base = %config.base_url,
                tasks = config.tasks.len(),
                "LLM client reloaded"
            );
        }
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        guard.config = config;
        guard.api_key = api_key;
    }

    pub fn has_api_key(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .api_key
            .is_some()
    }

    pub fn api_key_suffix(&self) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .api_key
            .as_deref()
            .and_then(api_key_suffix)
    }

    pub fn api_key_env(&self) -> String {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .api_key_env
            .clone()
    }

    pub fn snapshot_config(&self) -> LlmConfig {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .clone()
    }

    pub fn model_for_agent(&self, agent: &str) -> String {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = guard.config.agents.get(agent) {
            if let Some(m) = guard.config.tasks.get(task) {
                return m.clone();
            }
            return task.clone();
        }
        guard.config.default_model.clone()
    }

    /// Resolve max_tokens for an agent from llm.yaml task config.
    pub fn max_tokens_for_agent(&self, agent: &str) -> u32 {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = guard.config.agents.get(agent) {
            if let Some(&n) = guard.config.task_max_tokens.get(task) {
                return n.max(256);
            }
        }
        guard.config.default_max_tokens.max(256)
    }

    pub async fn complete(
        &self,
        system: &str,
        user: &str,
        model: Option<&str>,
    ) -> Result<String> {
        self.complete_limited(system, user, model, None).await
    }

    /// Like `complete`, but with an explicit max_tokens (e.g. writer chapter drafts).
    pub async fn complete_limited(
        &self,
        system: &str,
        user: &str,
        model: Option<&str>,
        max_tokens: Option<u32>,
    ) -> Result<String> {
        let result = self
            .complete_messages_limited(
                vec![
                    ChatMessage {
                        role: "system".into(),
                        content: system.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                },
                    ChatMessage {
                        role: "user".into(),
                        content: user.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                },
                ],
                model,
                None,
                max_tokens,
            )
            .await?;
        Ok(result.content)
    }

    /// Convenience: model + max_tokens from agent mapping.
    pub async fn complete_for_agent(
        &self,
        agent: &str,
        system: &str,
        user: &str,
    ) -> Result<String> {
        let model = self.model_for_agent(agent);
        let max = self.max_tokens_for_agent(agent);
        self.complete_limited(system, user, Some(&model), Some(max))
            .await
    }

    pub async fn complete_messages_limited(
        &self,
        messages: Vec<ChatMessage>,
        model: Option<&str>,
        tools: Option<&[ToolSpec]>,
        max_tokens: Option<u32>,
    ) -> Result<CompletionResult> {
        self.complete_messages_stream_limited(
            messages,
            model,
            tools,
            max_tokens,
            |_delta| async {},
        )
        .await
    }

    /// Stream chat completions; invokes `on_delta` for each content token/chunk.
    pub async fn complete_messages_stream<F, Fut>(
        &self,
        messages: Vec<ChatMessage>,
        model: Option<&str>,
        tools: Option<&[ToolSpec]>,
        on_delta: F,
    ) -> Result<CompletionResult>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        self.complete_messages_stream_limited(messages, model, tools, None, on_delta)
            .await
    }

    pub async fn complete_messages_stream_limited<F, Fut>(
        &self,
        messages: Vec<ChatMessage>,
        model: Option<&str>,
        tools: Option<&[ToolSpec]>,
        max_tokens: Option<u32>,
        mut on_delta: F,
    ) -> Result<CompletionResult>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        use std::sync::atomic::{AtomicBool, Ordering};

        let snap = {
            let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
            (
                guard.api_key.clone(),
                guard.config.default_model.clone(),
                guard
                    .config
                    .task_max_tokens
                    .get("studio")
                    .copied()
                    .unwrap_or(2048),
                guard.config.default_max_tokens,
                guard.config.base_url.clone(),
                guard.config.max_retries,
                guard.config.retry_base_delay_ms,
                guard.config.retry_max_delay_ms,
            )
        };
        let (
            key,
            default_model,
            studio_tokens,
            default_max_tokens,
            base_url,
            max_retries,
            retry_base,
            retry_max,
        ) = snap;
        let Some(key) = key else {
            let content = placeholder_reply(&messages);
            on_delta(content.clone()).await;
            return Ok(CompletionResult {
                content,
                tool_calls: vec![],
                reasoning_content: None,
            });
        };
        let model_owned = model.unwrap_or(&default_model).to_string();
        let model = model_owned.as_str();
        // Tool loops stay bounded; prose/creative uses yaml (default 8k).
        let tokens = max_tokens.unwrap_or(if tools.is_some() {
            studio_tokens
        } else {
            default_max_tokens
        });
        let messages = sanitize_chat_messages(messages);
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "max_tokens": tokens,
        });
        // DeepSeek V4 defaults thinking=enabled; CoT (`reasoning_content`) and the final
        // answer (`content`) share `max_tokens`. Non-tool calls with budget ≤64k keep
        // thinking off so JSON/patches land in `content` quickly. Large creative/planning
        // budgets (>64k) keep thinking for quality.
        let disable_thinking = tools.is_none() && tokens <= 65536;
        if disable_thinking {
            body["thinking"] = json!({"type": "disabled"});
        }
        if let Some(tools) = tools {
            let tools_json: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools_json);
        }
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let deadline_secs = if tokens >= 65536 {
            300
        } else if tokens >= 4096 {
            180
        } else {
            90
        };
        let started = std::time::Instant::now();
        tracing::info!(
            %model,
            max_tokens = tokens,
            deadline_secs,
            max_retries,
            url = %url,
            "llm stream start"
        );

        let mut attempt: u32 = 0;
        let (content, tool_calls, delta_events, reasoning_content) = loop {
            let emitted = AtomicBool::new(false);
            // Hard deadline — reqwest timeout alone can miss stuck body streams.
            let result = {
                let on_delta_ref = &mut on_delta;
                let emitted_ref = &emitted;
                let mut delta_cb = |piece: String| {
                    emitted_ref.store(true, Ordering::Relaxed);
                    on_delta_ref(piece)
                };
                tokio::time::timeout(
                    std::time::Duration::from_secs(deadline_secs),
                    self.drive_chat_stream(&key, &base_url, body.clone(), &mut delta_cb),
                )
                .await
            };
            let emitted_any = emitted.load(Ordering::Relaxed);
            match result {
                Ok(Ok(v)) => break v,
                Ok(Err(e)) => {
                    let msg = format!("{e:#}");
                    let can_retry = !emitted_any
                        && attempt < max_retries
                        && is_transient_llm_error(&msg);
                    if !can_retry {
                        return Err(e);
                    }
                    let delay = retry_delay_ms(attempt, retry_base, retry_max);
                    tracing::warn!(
                        %model,
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay,
                        error = %msg,
                        "llm transient error before first token; retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    attempt += 1;
                }
                Err(_) => {
                    let msg = format!(
                        "llm stream timeout after {deadline_secs}s (model={model}, max_tokens={tokens})"
                    );
                    let can_retry = !emitted_any && attempt < max_retries;
                    if !can_retry {
                        tracing::error!(
                            %model,
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            deadline_secs,
                            emitted_any,
                            "llm stream deadline exceeded"
                        );
                        anyhow::bail!(msg);
                    }
                    let delay = retry_delay_ms(attempt, retry_base, retry_max);
                    tracing::warn!(
                        %model,
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay,
                        "llm stream deadline with no tokens; retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    attempt += 1;
                }
            }
        };

        // Safety net: if thinking mode still filled only reasoning, surface that text so
        // JSON extractors (consistency auditor etc.) are not handed an empty string.
        let content = if content.trim().is_empty() {
            match &reasoning_content {
                Some(r) if !r.trim().is_empty() => {
                    tracing::warn!(
                        %model,
                        reasoning_chars = r.chars().count(),
                        "llm content empty; falling back to reasoning_content"
                    );
                    r.clone()
                }
                _ => content,
            }
        } else {
            content
        };

        tracing::info!(
            %model,
            max_tokens = tokens,
            thinking_disabled = disable_thinking,
            attempts = attempt + 1,
            elapsed_ms = started.elapsed().as_millis() as u64,
            content_chars = content.chars().count(),
            reasoning_chars = reasoning_content.as_ref().map(|s| s.chars().count()).unwrap_or(0),
            delta_events,
            tool_calls = tool_calls.len(),
            "llm stream done"
        );
        Ok(CompletionResult {
            content,
            tool_calls,
            reasoning_content,
        })
    }

    async fn drive_chat_stream<F, Fut>(
        &self,
        key: &str,
        base_url: &str,
        body: Value,
        on_delta: &mut F,
    ) -> Result<(String, Vec<ToolCall>, u32, Option<String>)>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .context("llm request")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("llm http {status}: {text}");
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut tool_acc: HashMap<u64, (String, String, String)> = HashMap::new();
        let mut delta_events: u32 = 0;
        // Time-to-first-token: headers OK but body silent → fail fast (was looking
        // like endless「仍在等待首包」).
        const TTFT_SECS: u64 = 60;
        let mut got_first_token = false;

        loop {
            let next = tokio::time::timeout(
                std::time::Duration::from_secs(if got_first_token { 120 } else { TTFT_SECS }),
                stream.next(),
            )
            .await;
            let chunk = match next {
                Ok(Some(c)) => c.context("llm stream chunk")?,
                Ok(None) => break,
                Err(_) if !got_first_token => {
                    anyhow::bail!(
                        "llm TTFT timeout after {TTFT_SECS}s (no first token; check API / rate limit)"
                    );
                }
                Err(_) => {
                    anyhow::bail!("llm stream idle timeout after 120s (no chunks)");
                }
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf = buf[pos + 1..].to_string();
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line.trim_start_matches("data:").trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                let delta = &v["choices"][0]["delta"];
                if let Some(piece) = delta["content"].as_str() {
                    if !piece.is_empty() {
                        got_first_token = true;
                        content.push_str(piece);
                        delta_events += 1;
                        // Forward provider chunks as-is. Micro-slicing + sleep caused
                        // WS/UI backpressure that stalled whole audit pipelines.
                        on_delta(piece.to_string()).await;
                    }
                }
                // Thinking mode streams reasoning_content; must be stored and echoed
                // back on subsequent tool-call turns (DeepSeek 400 otherwise).
                if let Some(piece) = delta["reasoning_content"].as_str() {
                    if !piece.is_empty() {
                        got_first_token = true;
                        reasoning.push_str(piece);
                    }
                }
                if let Some(arr) = delta["tool_calls"].as_array() {
                    if !arr.is_empty() {
                        got_first_token = true;
                    }
                    for tc in arr {
                        let idx = tc["index"].as_u64().unwrap_or(0);
                        let entry = tool_acc.entry(idx).or_insert_with(|| {
                            (String::new(), String::new(), String::new())
                        });
                        if let Some(id) = tc["id"].as_str() {
                            if !id.is_empty() {
                                entry.0 = id.to_string();
                            }
                        }
                        if let Some(name) = tc["function"]["name"].as_str() {
                            if !name.is_empty() {
                                entry.1.push_str(name);
                            }
                        }
                        if let Some(args) = tc["function"]["arguments"].as_str() {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }

        let mut indexed: Vec<(u64, ToolCall)> = tool_acc
            .into_iter()
            .map(|(idx, (id, name, arguments))| {
                (
                    idx,
                    ToolCall {
                        id: if id.is_empty() {
                            format!("call_{idx}")
                        } else {
                            id
                        },
                        name,
                        arguments: if arguments.is_empty() {
                            "{}".into()
                        } else {
                            arguments
                        },
                    },
                )
            })
            .collect();
        indexed.sort_by_key(|(idx, _)| *idx);
        let mut tool_calls: Vec<ToolCall> = indexed.into_iter().map(|(_, tc)| tc).collect();

        if tool_calls.is_empty() {
            let (parsed, cleaned) = extract_tool_calls_from_text(&content);
            if !parsed.is_empty() {
                tool_calls = parsed;
                content = cleaned;
            }
        }

        let reasoning_content = if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        };
        Ok((content, tool_calls, delta_events, reasoning_content))
    }

    pub async fn complete_stream_limited<F, Fut>(
        &self,
        system: &str,
        user: &str,
        model: Option<&str>,
        max_tokens: Option<u32>,
        mut on_delta: F,
    ) -> Result<String>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let result = self
            .complete_messages_stream_limited(
                vec![
                    ChatMessage {
                        role: "system".into(),
                        content: system.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                },
                    ChatMessage {
                        role: "user".into(),
                        content: user.into(),
                        tool_call_id: None,
                        tool_calls: None,
                    ..Default::default()
                },
                ],
                model,
                None,
                max_tokens,
                |d| on_delta(d),
            )
            .await?;
        Ok(result.content)
    }
}

/// Transient transport / upstream faults worth retrying (before any stream token).
pub fn is_transient_llm_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    // Auth / client mistakes — never retry.
    if m.contains("missing api key")
        || m.contains("invalid_api_key")
        || m.contains("authentication")
        || m.contains("llm http 400")
        || m.contains("llm http 401")
        || m.contains("llm http 403")
        || m.contains("llm http 404")
        || m.contains("llm http 422")
    {
        return false;
    }
    // Rate limit / upstream blips.
    if m.contains("llm http 408")
        || m.contains("llm http 429")
        || m.contains("llm http 500")
        || m.contains("llm http 502")
        || m.contains("llm http 503")
        || m.contains("llm http 504")
        || m.contains("rate limit")
        || m.contains("overloaded")
        || m.contains("capacity")
    {
        return true;
    }
    // Network / connect / early timeouts (TTFT, connect). Mid-stream idle timeouts also
    // match "timeout", but callers only retry when no token was emitted.
    if m.contains("llm request")
        || m.contains("error sending request")
        || m.contains("connection")
        || m.contains("connect")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("ttft")
        || m.contains("broken pipe")
        || m.contains("reset")
        || m.contains("dns")
        || m.contains("temporarily unavailable")
        || m.contains("network")
    {
        return true;
    }
    false
}

fn retry_delay_ms(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    let base = base_ms.max(100);
    let max = max_ms.max(base);
    let exp = base.saturating_mul(1u64 << attempt.min(6));
    // Tiny deterministic jitter so parallel clients don't sync-thump.
    let jitter = (attempt as u64).saturating_mul(37) % (base / 4).max(1);
    exp.saturating_add(jitter).min(max)
}

fn placeholder_reply(messages: &[ChatMessage]) -> String {
    let user = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    format!(
        "（未配置 API Key，占位回复）已收到：{}",
        user.chars().take(120).collect::<String>()
    )
}

/// Repair chat history so DeepSeek/OpenAI accept it:
/// - drop / demote orphan `tool` messages (no preceding assistant `tool_calls`)
/// - keep only `tool` replies whose `tool_call_id` matches that round
/// - synthesize missing `tool` results for unanswered `tool_calls`
/// - collapse tool rounds that lack `reasoning_content` (DeepSeek thinking mode 400)
pub fn sanitize_chat_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len() + 4);
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];

        // Orphan tool (history truncated / gate race) → keep content as assistant prose.
        if msg.role == "tool" {
            let content = msg.content.trim();
            if !content.is_empty() {
                out.push(ChatMessage {
                    role: "assistant".into(),
                    content: msg.content.clone(),
                    tool_call_id: None,
                    tool_calls: None,
                    ..Default::default()
                });
            }
            i += 1;
            continue;
        }

        let tool_ids = tool_call_ids(msg);
        if tool_ids.is_empty() {
            out.push(msg.clone());
            i += 1;
            continue;
        }

        // Collect following tool results first.
        i += 1;
        let expected: std::collections::HashSet<String> = tool_ids.iter().cloned().collect();
        let mut answered = std::collections::HashSet::new();
        let mut tool_msgs: Vec<ChatMessage> = Vec::new();
        let mut demoted: Vec<String> = Vec::new();
        while i < messages.len() && messages[i].role == "tool" {
            let id = messages[i].tool_call_id.clone().unwrap_or_default();
            if expected.contains(&id) && answered.insert(id) {
                tool_msgs.push(messages[i].clone());
            } else if !messages[i].content.trim().is_empty() {
                demoted.push(messages[i].content.clone());
            }
            i += 1;
        }
        for id in &tool_ids {
            if answered.contains(id) {
                continue;
            }
            tool_msgs.push(ChatMessage {
                role: "tool".into(),
                content: "（已跳过：等待用户确认或本轮提前结束）".into(),
                tool_call_id: Some(id.clone()),
                tool_calls: None,
                ..Default::default()
            });
        }

        let has_reasoning = msg
            .reasoning_content
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_reasoning {
            out.push(msg.clone());
            out.extend(tool_msgs);
        } else {
            // Stale history (pre-fix) cannot be replayed in thinking+tools mode.
            let mut collapsed = msg.content.clone();
            if collapsed.trim().is_empty() {
                collapsed = "（已执行工具）".into();
            }
            for tm in &tool_msgs {
                if !tm.content.trim().is_empty() {
                    collapsed.push_str("\n\n");
                    collapsed.push_str(tm.content.trim());
                }
            }
            out.push(ChatMessage {
                role: "assistant".into(),
                content: collapsed,
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            });
        }
        for content in demoted {
            out.push(ChatMessage {
                role: "assistant".into(),
                content,
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            });
        }
    }
    out
}

fn tool_call_ids(msg: &ChatMessage) -> Vec<String> {
    if msg.role != "assistant" {
        return vec![];
    }
    let Some(Value::Array(arr)) = msg.tool_calls.as_ref() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|tc| {
            tc.get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
        .collect()
}

/// Parse XML-style / markdown tool calls that models sometimes put in content.
/// Supports:
/// ```text
/// <invoke name="audit_chapter">
///   <parameter name="project_id">demo</parameter>
///   <parameter name="chapter_number">1</parameter>
/// </invoke>
/// ```
pub fn extract_tool_calls_from_text(content: &str) -> (Vec<ToolCall>, String) {
    use regex::Regex;
    use std::sync::OnceLock;

    static RE_INVOKE: OnceLock<Regex> = OnceLock::new();
    let re = RE_INVOKE.get_or_init(|| {
        Regex::new(
            r#"(?s)<invoke\s+name=["']([^"']+)["']\s*>(.*?)</invoke>"#,
        )
        .unwrap()
    });
    static RE_PARAM: OnceLock<Regex> = OnceLock::new();
    let re_param = RE_PARAM.get_or_init(|| {
        Regex::new(
            r#"(?s)<parameter\s+name=["']([^"']+)["']\s*>(.*?)</parameter>"#,
        )
        .unwrap()
    });

    let mut calls = Vec::new();
    for (i, caps) in re.captures_iter(content).enumerate() {
        let name = caps[1].trim().to_string();
        let body = &caps[2];
        let mut args = serde_json::Map::new();
        for p in re_param.captures_iter(body) {
            let key = normalize_arg_key(p[1].trim());
            let val = p[2].trim().to_string();
            if let Ok(n) = val.parse::<u64>() {
                args.insert(key, json!(n));
            } else if val == "true" || val == "false" {
                args.insert(key, json!(val == "true"));
            } else {
                args.insert(key, json!(val));
            }
        }
        calls.push(ToolCall {
            id: format!("xml_call_{i}"),
            name,
            arguments: Value::Object(args).to_string(),
        });
    }

    let cleaned = if calls.is_empty() {
        content.to_string()
    } else {
        let mut t = content.to_string();
        // Strip common wrappers so UI doesn't show raw tool markup
        for pat in [
            r"(?s)<function_calls>.*?</function_calls>",
            r#"(?s)<invoke\s+name=["'][^"']+["']\s*>.*?</invoke>"#,
        ] {
            if let Ok(re_strip) = Regex::new(pat) {
                t = re_strip.replace_all(&t, "").to_string();
            }
        }
        t.trim().to_string()
    };
    (calls, cleaned)
}

fn normalize_arg_key(key: &str) -> String {
    match key {
        "project_id" | "project_name" | "novel" | "name" => "project".into(),
        "chapter_number" | "chapter_num" | "ch" | "n" => "chapter".into(),
        "user_instructions" | "instruction" | "msg" | "message" => "instructions".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-mutating tests across threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn api_key_suffix_takes_last_four() {
        assert_eq!(api_key_suffix("sk-abcdefgh").as_deref(), Some("efgh"));
        assert_eq!(api_key_suffix("ab").as_deref(), Some("ab"));
        assert_eq!(api_key_suffix("   ").as_deref(), None);
    }

    #[test]
    fn reload_updates_model_and_key_status() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let env = "NOVELX_TEST_LLM_KEY_RELOAD";
        unsafe { std::env::remove_var(env) };
        let mut cfg = LlmConfig::default();
        cfg.api_key_env = env.into();
        cfg.default_model = "model-a".into();
        cfg.agents.insert("writer".into(), "creative".into());
        cfg.tasks.insert("creative".into(), "model-a".into());
        let client = LlmClient::new(cfg);
        assert!(!client.has_api_key());
        assert_eq!(client.model_for_agent("writer"), "model-a");

        unsafe { std::env::set_var(env, "sk-test-secret-key-9999") };
        let mut cfg2 = LlmConfig::default();
        cfg2.api_key_env = env.into();
        cfg2.default_model = "model-b".into();
        cfg2.agents.insert("writer".into(), "creative".into());
        cfg2.tasks.insert("creative".into(), "model-b".into());
        client.reload(cfg2);
        assert!(client.has_api_key());
        assert_eq!(client.api_key_suffix().as_deref(), Some("9999"));
        assert_eq!(client.model_for_agent("writer"), "model-b");
        unsafe { std::env::remove_var(env) };
    }

    #[test]
    fn upsert_dotenv_key_roundtrip_without_exposing_in_errors() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "novelx-llm-dotenv-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        let env_name = "NOVELX_TEST_DOTENV_KEY";
        std::fs::write(&path, format!("FOO=1\n{env_name}=old\nBAR=2\n")).unwrap();
        upsert_dotenv_key(&path, env_name, "sk-new-secret-abcd").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("{env_name}=sk-new-secret-abcd")));
        assert!(text.contains("FOO=1"));
        assert!(text.contains("BAR=2"));
        assert_eq!(
            std::env::var(env_name).ok().as_deref(),
            Some("sk-new-secret-abcd")
        );
        unsafe { std::env::remove_var(env_name) };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_form_preserves_agents_and_temperature() {
        let existing = r#"
profile: prod
dev_model: deepseek-v4-flash
providers:
  deepseek:
    api_key_env: DEEPSEEK_API_KEY
    base_url: https://api.deepseek.com
tasks:
  creative:
    description: 正文
    provider: deepseek
    model: deepseek-v4-pro
    temperature: 0.85
    max_tokens: 131072
agents:
  writer: creative
"#;
        let put = LlmFormPut {
            profile: "prod".into(),
            dev_model: "deepseek-v4-flash".into(),
            default_provider: "deepseek".into(),
            providers: vec![LlmProviderForm {
                name: "deepseek".into(),
                base_url: Some("https://api.deepseek.com".into()),
                api_key_env: "DEEPSEEK_API_KEY".into(),
                has_api_key: false,
                api_key_suffix: None,
            }],
            tasks: vec![LlmTaskForm {
                name: "creative".into(),
                description: "正文".into(),
                model: "deepseek-v4-flash".into(),
                max_tokens: 65536,
            }],
            retry: LlmRetryForm {
                max_retries: 3,
                base_delay_ms: 800,
                max_delay_ms: 10000,
            },
            api_key: None,
        };
        validate_llm_form_put(&put).unwrap();
        let out = apply_llm_form_to_yaml(existing, &put).unwrap();
        assert!(out.contains("temperature"));
        assert!(out.contains("writer"));
        assert!(out.contains("deepseek-v4-flash"));
        assert!(out.contains("65536"));
    }

    #[test]
    fn transient_errors_are_retryable() {
        assert!(is_transient_llm_error("llm http 429: rate limit"));
        assert!(is_transient_llm_error("llm http 503: overloaded"));
        assert!(is_transient_llm_error("llm request: error sending request"));
        assert!(is_transient_llm_error(
            "llm TTFT timeout after 60s (no first token; check API / rate limit)"
        ));
        assert!(is_transient_llm_error("connection reset by peer"));
    }

    #[test]
    fn permanent_errors_are_not_retryable() {
        assert!(!is_transient_llm_error("llm http 401: invalid_api_key"));
        assert!(!is_transient_llm_error("llm http 400: invalid messages"));
        assert!(!is_transient_llm_error("missing api key"));
        assert!(!is_transient_llm_error("parse: bad json"));
    }

    #[test]
    fn retry_delay_caps_and_grows() {
        let d0 = retry_delay_ms(0, 800, 10_000);
        let d1 = retry_delay_ms(1, 800, 10_000);
        let d5 = retry_delay_ms(5, 800, 10_000);
        assert!(d0 >= 800 && d0 <= 10_000);
        assert!(d1 > d0);
        assert_eq!(d5, 10_000);
    }

    #[test]
    fn parse_xml_invokes() {
        let text = r#"正在审校
<function_calls>
<invoke name="audit_chapter">
<parameter name="project_id">demo</parameter>
<parameter name="chapter_number">1</parameter>
</invoke>
<invoke name="audit_chapter">
<parameter name="project_id">demo</parameter>
<parameter name="chapter_number">2</parameter>
</invoke>
</function_calls>"#;
        let (calls, cleaned) = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "audit_chapter");
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["project"], "demo");
        assert_eq!(args["chapter"], 1);
        assert!(!cleaned.contains("<invoke"));
        assert!(cleaned.contains("正在审校"));
    }

    #[test]
    fn sanitize_fills_missing_tool_results() {
        let msgs = vec![
            ChatMessage {
                role: "user".into(),
                content: "审校".into(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(json!([
                    {"id":"c1","type":"function","function":{"name":"audit_chapter","arguments":"{}"}},
                    {"id":"c2","type":"function","function":{"name":"list_projects","arguments":"{}"}}
                ])),
                reasoning_content: Some("need tools".into()),
            },
            ChatMessage {
                role: "tool".into(),
                content: "audit done".into(),
                tool_call_id: Some("c1".into()),
                tool_calls: None,
                ..Default::default()
            },
            ChatMessage {
                role: "user".into(),
                content: "修正".into(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            },
        ];
        let fixed = sanitize_chat_messages(msgs);
        let tool_ids: Vec<_> = fixed
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();
        assert!(tool_ids.contains(&"c1".into()));
        assert!(tool_ids.contains(&"c2".into()));
        assert_eq!(fixed.last().unwrap().content, "修正");
    }

    #[test]
    fn sanitize_collapses_tool_rounds_missing_reasoning() {
        let msgs = vec![
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(json!([{
                    "id":"c1","type":"function",
                    "function":{"name":"list_projects","arguments":"{}"}
                }])),
                reasoning_content: None,
            },
            ChatMessage {
                role: "tool".into(),
                content: "projects ok".into(),
                tool_call_id: Some("c1".into()),
                tool_calls: None,
                ..Default::default()
            },
        ];
        let fixed = sanitize_chat_messages(msgs);
        assert!(fixed.iter().all(|m| m.role != "tool"));
        assert!(fixed[0].tool_calls.is_none());
        assert!(fixed[0].content.contains("projects ok"));
    }

    #[test]
    fn sanitize_demotes_orphan_tool_messages() {
        let msgs = vec![
            ChatMessage {
                role: "tool".into(),
                content: "孤儿工具结果".into(),
                tool_call_id: Some("orphan".into()),
                tool_calls: None,
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".into(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: Some(json!([{
                    "id":"direct_steer_run","type":"function",
                    "function":{"name":"steer_run","arguments":"{}"}
                }])),
                reasoning_content: Some("steer".into()),
            },
            ChatMessage {
                role: "tool".into(),
                content: "steer ok".into(),
                tool_call_id: Some("direct_steer_run".into()),
                tool_calls: None,
                ..Default::default()
            },
            ChatMessage {
                role: "user".into(),
                content: "继续".into(),
                tool_call_id: None,
                tool_calls: None,
                ..Default::default()
            },
        ];
        let fixed = sanitize_chat_messages(msgs);
        assert_eq!(fixed[0].role, "assistant");
        assert_eq!(fixed[0].content, "孤儿工具结果");
        assert!(fixed[0].tool_call_id.is_none());
        assert_eq!(fixed[1].role, "assistant");
        assert!(fixed[1].tool_calls.is_some());
        assert_eq!(fixed[1].reasoning_content.as_deref(), Some("steer"));
        assert_eq!(fixed[2].role, "tool");
        assert_eq!(fixed[2].tool_call_id.as_deref(), Some("direct_steer_run"));
        assert_eq!(fixed.last().unwrap().content, "继续");
        for (i, m) in fixed.iter().enumerate() {
            if m.role != "tool" {
                continue;
            }
            assert!(i > 0);
            assert!(!tool_call_ids(&fixed[i - 1]).is_empty() || fixed[i - 1].role == "tool");
        }
    }
}
