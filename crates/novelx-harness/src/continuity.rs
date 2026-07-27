//! Load `config/continuity.yaml` — long-horizon CanonContext / recall budgets.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ContinuityTier {
    /// Inclusive upper bound on published_count; `0` means unlimited (fallback tier).
    #[serde(default)]
    pub max_published: u32,
    #[serde(default = "default_memory_section")]
    pub memory_section: usize,
    #[serde(default = "default_dangling")]
    pub dangling_show: usize,
    #[serde(default = "default_archive_recall")]
    pub archive_thread_recall: usize,
    #[serde(default = "default_summary_recall")]
    pub summary_recall: usize,
    #[serde(default = "default_asserted")]
    pub asserted_facts: usize,
    #[serde(default = "default_rollups")]
    pub volume_rollups: usize,
    #[serde(default = "default_bridge")]
    pub bridge_chars: usize,
}

fn default_memory_section() -> usize {
    2200
}
fn default_dangling() -> usize {
    10
}
fn default_archive_recall() -> usize {
    3
}
fn default_summary_recall() -> usize {
    4
}
fn default_asserted() -> usize {
    8
}
fn default_rollups() -> usize {
    3
}
fn default_bridge() -> usize {
    900
}

#[derive(Debug, Clone, Deserialize)]
struct ContinuityFile {
    #[serde(default)]
    tiers: Vec<ContinuityTierRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct ContinuityTierRaw {
    #[serde(default)]
    max_published: u32,
    #[serde(default = "default_memory_section")]
    memory_section: usize,
    #[serde(default = "default_dangling")]
    dangling_show: usize,
    #[serde(default = "default_archive_recall")]
    archive_thread_recall: usize,
    #[serde(default = "default_summary_recall")]
    summary_recall: usize,
    #[serde(default = "default_asserted")]
    asserted_facts: usize,
    #[serde(default = "default_rollups")]
    volume_rollups: usize,
    #[serde(default = "default_bridge")]
    bridge_chars: usize,
}

#[derive(Debug, Clone)]
pub struct ContinuityBudget {
    tiers: Vec<ContinuityTier>,
}

impl Default for ContinuityBudget {
    fn default() -> Self {
        Self {
            tiers: vec![
                ContinuityTier {
                    max_published: 30,
                    memory_section: 2000,
                    dangling_show: 8,
                    archive_thread_recall: 2,
                    summary_recall: 2,
                    asserted_facts: 6,
                    volume_rollups: 2,
                    bridge_chars: 900,
                },
                ContinuityTier {
                    max_published: 120,
                    memory_section: 2400,
                    dangling_show: 10,
                    archive_thread_recall: 3,
                    summary_recall: 4,
                    asserted_facts: 8,
                    volume_rollups: 3,
                    bridge_chars: 1000,
                },
                ContinuityTier {
                    max_published: 0,
                    memory_section: 2800,
                    dangling_show: 12,
                    archive_thread_recall: 4,
                    summary_recall: 6,
                    asserted_facts: 10,
                    volume_rollups: 4,
                    bridge_chars: 1100,
                },
            ],
        }
    }
}

impl ContinuityBudget {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_yaml::from_str::<ContinuityFile>(&text) {
            Ok(f) if !f.tiers.is_empty() => Self {
                tiers: f
                    .tiers
                    .into_iter()
                    .map(|t| ContinuityTier {
                        max_published: t.max_published,
                        memory_section: t.memory_section.max(800),
                        dangling_show: t.dangling_show.max(2),
                        archive_thread_recall: t.archive_thread_recall,
                        summary_recall: t.summary_recall.max(1),
                        asserted_facts: t.asserted_facts,
                        volume_rollups: t.volume_rollups.max(1),
                        bridge_chars: t.bridge_chars.max(400),
                    })
                    .collect(),
            },
            Ok(_) => Self::default(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse continuity.yaml");
                Self::default()
            }
        }
    }

    pub fn load_from_config_root(config_root: &Path) -> Self {
        Self::load(&config_root.join("continuity.yaml"))
    }

    /// Pick tier for `published_count` (first matching max_published, else last).
    pub fn for_published(&self, published_count: u32) -> &ContinuityTier {
        for t in &self.tiers {
            if t.max_published == 0 {
                continue;
            }
            if published_count <= t.max_published {
                return t;
            }
        }
        self.tiers.last().unwrap_or(&self.tiers[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_scale_with_published() {
        let b = ContinuityBudget::default();
        assert_eq!(b.for_published(10).summary_recall, 2);
        assert_eq!(b.for_published(50).summary_recall, 4);
        assert_eq!(b.for_published(200).summary_recall, 6);
    }

    #[test]
    fn loads_yaml() {
        let dir = std::env::temp_dir().join("novelx-continuity-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("continuity.yaml");
        std::fs::write(
            &path,
            "tiers:\n  - max_published: 5\n    summary_recall: 3\n  - max_published: 0\n    summary_recall: 7\n",
        )
        .unwrap();
        let b = ContinuityBudget::load(&path);
        assert_eq!(b.for_published(2).summary_recall, 3);
        assert_eq!(b.for_published(99).summary_recall, 7);
    }
}
