//! Config-driven studio policies (`config/policies.yaml`).
//! Keep Chinese phrase tables out of `lib.rs` / pipeline match arms.

use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
struct PoliciesFile {
    #[serde(default)]
    bare_continue: BareContinuePolicy,
    #[serde(default)]
    clear_history_on_write: ClearHistoryPolicy,
    #[serde(default)]
    full_rewrite: KeywordPolicy,
    #[serde(default)]
    volume_sync_manual: KeywordPolicy,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct BareContinuePolicy {
    #[serde(default)]
    phrases: Vec<String>,
    #[serde(default = "default_draft_min")]
    draft_min_chars: usize,
}

fn default_draft_min() -> usize {
    400
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ClearHistoryPolicy {
    #[serde(default)]
    exact: Vec<String>,
    #[serde(default)]
    any_keywords: Vec<String>,
    #[serde(default)]
    all_keyword_groups: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeywordPolicy {
    #[serde(default)]
    any_keywords: Vec<String>,
    #[serde(default)]
    all_keyword_groups: Vec<Vec<String>>,
    #[serde(default)]
    none_keywords: Vec<String>,
}

/// Loaded studio policies — shared by core + pipeline.
#[derive(Debug, Clone)]
pub struct StudioPolicies {
    inner: Arc<PoliciesFile>,
}

impl StudioPolicies {
    pub fn load(config_root: &Path) -> Self {
        let path = config_root.join("policies.yaml");
        if !path.exists() {
            tracing::warn!(
                path = %path.display(),
                "policies.yaml missing; using built-in defaults"
            );
            return Self::defaults();
        }
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_yaml::from_str::<PoliciesFile>(&raw) {
                Ok(file) => {
                    tracing::info!("studio policies loaded");
                    Self {
                        inner: Arc::new(file),
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "policies.yaml parse failed; using defaults");
                    Self::defaults()
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "policies.yaml read failed; using defaults");
                Self::defaults()
            }
        }
    }

    pub fn defaults() -> Self {
        let yaml = include_str!("../../../config/policies.yaml");
        let file: PoliciesFile =
            serde_yaml::from_str(yaml).expect("embedded policies.yaml must parse");
        Self {
            inner: Arc::new(file),
        }
    }

    pub fn draft_min_chars(&self) -> usize {
        self.inner.bare_continue.draft_min_chars.max(1)
    }

    pub fn is_bare_continue(&self, user_text: &str) -> bool {
        let t = user_text.trim();
        if t.is_empty() {
            return false;
        }
        self.inner
            .bare_continue
            .phrases
            .iter()
            .any(|p| p == t)
    }

    /// User clearly starts a new chapter write (safe to clear prior Studio chat).
    /// `chapter_named`: caller already parsed「第N章」from user text.
    pub fn user_intends_new_chapter_write(&self, user_text: &str, chapter_named: bool) -> bool {
        let t = user_text.trim();
        if t.is_empty() || self.is_bare_continue(t) {
            return false;
        }
        if chapter_named {
            return true;
        }
        let p = &self.inner.clear_history_on_write;
        if p.exact.iter().any(|x| x == t) {
            return true;
        }
        keyword_policy_hit(
            &KeywordPolicy {
                any_keywords: p.any_keywords.clone(),
                all_keyword_groups: p.all_keyword_groups.clone(),
                none_keywords: vec![],
            },
            t,
        )
    }

    pub fn needs_full_rewrite(&self, message: &str) -> bool {
        keyword_policy_hit(&self.inner.full_rewrite, message)
    }

    pub fn is_manual_volume_sync(&self, text: &str) -> bool {
        keyword_policy_hit(&self.inner.volume_sync_manual, text)
    }
}

fn keyword_policy_hit(policy: &KeywordPolicy, text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    let t_lower = t.to_lowercase();
    if policy
        .none_keywords
        .iter()
        .any(|k| contains_ci(t, &t_lower, k))
    {
        return false;
    }
    let any_ok = policy.any_keywords.is_empty()
        || policy
            .any_keywords
            .iter()
            .any(|k| contains_ci(t, &t_lower, k));
    let groups_ok = policy.all_keyword_groups.is_empty()
        || policy.all_keyword_groups.iter().any(|group| {
            !group.is_empty() && group.iter().all(|k| contains_ci(t, &t_lower, k))
        });
    if !policy.any_keywords.is_empty() && !policy.all_keyword_groups.is_empty() {
        return policy
            .any_keywords
            .iter()
            .any(|k| contains_ci(t, &t_lower, k))
            || policy.all_keyword_groups.iter().any(|group| {
                !group.is_empty() && group.iter().all(|k| contains_ci(t, &t_lower, k))
            });
    }
    any_ok && groups_ok
}

fn contains_ci(t: &str, t_lower: &str, key: &str) -> bool {
    if key.bytes().all(|b| b.is_ascii()) {
        t_lower.contains(&key.to_lowercase())
    } else {
        t.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_continue_and_clear_history_from_defaults() {
        let p = StudioPolicies::defaults();
        assert!(p.is_bare_continue("继续"));
        assert!(!p.is_bare_continue("写第7章"));
        assert!(!p.user_intends_new_chapter_write("继续", false));
        assert!(p.user_intends_new_chapter_write("写第7章", true));
        assert!(p.user_intends_new_chapter_write("继续创作", false));
        assert!(p.needs_full_rewrite("请扩写到3000字"));
        assert!(!p.needs_full_rewrite("改一下第3段语气"));
        assert!(p.is_manual_volume_sync("同步第1卷设定"));
        assert!(!p.is_manual_volume_sync("审校第1卷"));
    }
}
