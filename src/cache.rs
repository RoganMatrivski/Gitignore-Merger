use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gitignore::{PrefixedRule, Rule};

const CACHE_VERSION: u32 = 2; // bumped from 1 — new schema (added rules field)
const CACHE_FILENAME: &str = "_dir-processor-cache.json";

// ---------------------------------------------------------------------------
// Serialisable mirror of PrefixedRule
//
// PrefixedRule owns non-Serialize types (PathBuf, custom Rule), so we store
// a flat, fully-serialisable mirror and convert on the way in/out.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CachedRule {
    pub pattern: String,
    pub negated: bool,
    pub dir_only: bool,
    /// `relative_dir` as a forward-slash string so the cache is portable
    /// across OSes (Windows uses `\` in PathBuf display).
    pub relative_dir: String,
}

impl CachedRule {
    pub fn from_prefixed(r: &PrefixedRule) -> Self {
        // Normalise path separators to `/` for portability
        let relative_dir = r.relative_dir.to_string_lossy().replace('\\', "/");
        Self {
            pattern: r.rule.pattern.clone(),
            negated: r.rule.negated,
            dir_only: r.rule.dir_only,
            relative_dir,
        }
    }

    pub fn into_prefixed(self) -> PrefixedRule {
        PrefixedRule {
            rule: Rule {
                pattern: self.pattern,
                negated: self.negated,
                dir_only: self.dir_only,
            },
            // Re-parse the portable `/`-separated string back into a PathBuf.
            // On Windows, PathBuf::from("/foo/bar") still works for relative paths.
            relative_dir: PathBuf::from(self.relative_dir),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-directory cache entry
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CacheEntry {
    /// Hash of direct children metadata (names + sizes + mtimes).
    pub shallow_fp: String,

    /// Hash of shallow_fp + all children's deep_fps.
    /// A change anywhere in the subtree changes this value.
    pub deep_fp: String,

    /// Parsed rules from the `.gitignore` file in this directory, if one exists.
    /// `None` means no `.gitignore` here (not a cache miss — explicitly absent).
    /// On a deep_fp hit we return these directly, skipping read_gitignore.
    pub rules: Option<Vec<CachedRule>>,
}

// ---------------------------------------------------------------------------
// Cache file wrapper
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CacheEntry>,
}

// ---------------------------------------------------------------------------
// Cache handle
// ---------------------------------------------------------------------------

pub struct Cache {
    entries: HashMap<String, CacheEntry>,
    path: PathBuf,
    dirty: bool,
}

impl Cache {
    /// Load from `root/_dir-processor-cache.json`.
    /// Returns empty on first run, version mismatch, or corrupt file.
    pub fn load(root: &Path) -> eyre::Result<Self> {
        let path = root.join(CACHE_FILENAME);

        if path.exists() {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<CacheFile>(&s).ok())
            {
                Some(f) if f.version == CACHE_VERSION => {
                    tracing::debug!(
                        "Loaded {} cache entries from {}",
                        f.entries.len(),
                        path.display()
                    );
                    return Ok(Self {
                        entries: f.entries,
                        path,
                        dirty: false,
                    });
                }
                _ => {
                    tracing::warn!(
                        "Cache at {} is outdated or corrupt — starting fresh",
                        path.display()
                    );
                }
            }
        }

        Ok(Self {
            entries: HashMap::new(),
            path,
            dirty: false,
        })
    }

    /// Write back to disk only when something changed this run.
    pub fn save(&self) -> eyre::Result<()> {
        if !self.dirty {
            tracing::debug!("Cache unchanged, skipping write");
            return Ok(());
        }
        let file = CacheFile {
            version: CACHE_VERSION,
            entries: self.entries.clone(),
        };
        let text = serde_json::to_string_pretty(&file)?;
        std::fs::write(&self.path, text)?;
        tracing::debug!(
            "Saved {} cache entries to {}",
            self.entries.len(),
            self.path.display()
        );
        Ok(())
    }

    pub fn get(&self, dir: &Path) -> Option<&CacheEntry> {
        self.entries.get(&cache_key(dir))
    }

    pub fn set(&mut self, dir: &Path, entry: CacheEntry) {
        self.entries.insert(cache_key(dir), entry);
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cache_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}
