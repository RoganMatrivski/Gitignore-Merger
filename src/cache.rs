use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CACHE_VERSION: u32 = 1;
const CACHE_FILENAME: &str = "_dir-processor-cache.json";

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CacheEntry {
    /// Fingerprint of this dir's direct children (names + sizes + mtimes).
    /// Cheap to recompute — just stat the immediate children.
    pub shallow_fp: String,

    /// hash(shallow_fp | child_0_deep | child_1_deep | …)
    /// Encodes the state of the entire subtree.  A change anywhere below
    /// propagates up through this value without a full rescan.
    pub deep_fp: String,
}

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
    /// Load the cache stored at `root/_dir-processor-cache.json`.
    /// Returns an empty cache on first run or if the file is corrupt/outdated.
    pub fn load(root: &Path) -> eyre::Result<Self> {
        let path = root.join(CACHE_FILENAME);

        if path.exists() {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<CacheFile>(&s).ok())
            {
                Some(file) if file.version == CACHE_VERSION => {
                    tracing::debug!("Loaded {} cache entries from {}", file.entries.len(), path.display());
                    return Ok(Self { entries: file.entries, path, dirty: false });
                }
                _ => {
                    tracing::warn!("Cache at {} is outdated or corrupt — starting fresh", path.display());
                }
            }
        }

        Ok(Self { entries: HashMap::new(), path, dirty: false })
    }

    /// Write the cache back to disk if it was modified this run.
    /// Skips the write if nothing changed (no wasted I/O on a clean run).
    pub fn save(&self) -> eyre::Result<()> {
        if !self.dirty {
            tracing::debug!("Cache unchanged, skipping write");
            return Ok(());
        }

        let file = CacheFile { version: CACHE_VERSION, entries: self.entries.clone() };
        let text = serde_json::to_string_pretty(&file)?;
        std::fs::write(&self.path, text)?;
        tracing::debug!("Saved {} cache entries to {}", self.entries.len(), self.path.display());
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

/// Stable, platform-normalised key for a path.
///
/// - Canonicalize to resolve symlinks and `..` segments.
/// - Lowercase because Windows paths are case-insensitive.
/// - Fall back to the raw path string if canonicalize fails (path doesn't exist yet).
fn cache_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase()
}
