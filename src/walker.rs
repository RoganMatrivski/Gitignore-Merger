use std::path::{Path, PathBuf};

use eyre::eyre;

use crate::cache::{Cache, CacheEntry};
use crate::fingerprint::{compute_deep_fp, fingerprint_dir};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Summary of what changed during a walk.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    /// True if anything in the tree changed since the last run.
    pub any_changed: bool,

    /// The specific dirs whose `deep_fp` differed from cache.
    /// Useful for logging / targeted reprocessing.
    pub changed_dirs: Vec<PathBuf>,
}

/// Walk `root` recursively, fingerprinting every directory and comparing
/// against the on-disk cache.
///
/// Returns which dirs changed.  The caller decides what to do with that
/// information (skip processing entirely, re-read only changed subtrees, etc.)
///
/// The walk itself is always O(number of dirs) in stat calls — no file
/// contents are read here.  Only dirs whose `deep_fp` differs from cache
/// are marked changed.
pub fn walk_cached(root: &Path, cache: &mut Cache) -> eyre::Result<WalkOutcome> {
    let mut outcome = WalkOutcome::default();
    walk_dir(root, cache, &mut outcome)?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Recurse into `dir`, update the cache, and record any changed dirs.
/// Returns the deep fingerprint of this dir so the parent can include it.
fn walk_dir(
    dir: &Path,
    cache: &mut Cache,
    outcome: &mut WalkOutcome,
) -> eyre::Result<String> {
    // --- 1. Shallow fingerprint (cheap: just stat direct children) -----------
    let shallow_fp = fingerprint_dir(dir).unwrap_or_else(|e| {
        tracing::warn!("Cannot fingerprint {}: {e}", dir.display());
        String::new() // treat as "always changed" — safe fallback
    });

    // --- 2. Recurse into subdirectories -------------------------------------
    //
    // We always recurse rather than trusting a cached deep_fp, because on
    // Linux/Windows modifying a file inside a subdir does NOT update the
    // parent dir's mtime.  Computing deep_fp bottom-up is the only reliable
    // cross-platform way to propagate changes upward.
    //
    // The cost is one `read_dir` call per directory (the fingerprint_dir above),
    // which is much cheaper than re-reading file contents.
    let subdirs = list_subdirs(dir)?;

    let mut child_deep_fps: Vec<String> = Vec::with_capacity(subdirs.len());
    for subdir in &subdirs {
        let child_deep = walk_dir(subdir, cache, outcome)?;
        child_deep_fps.push(child_deep);
    }

    // --- 3. Deep fingerprint (this dir + entire subtree) --------------------
    let deep_fp = compute_deep_fp(&shallow_fp, &child_deep_fps);

    // --- 4. Compare against cache -------------------------------------------
    let cached_deep = cache.get(dir).map(|e| e.deep_fp.as_str().to_owned());

    match cached_deep {
        Some(ref cached) if cached == &deep_fp => {
            tracing::debug!("Unchanged  {}", dir.display());
        }
        Some(_) => {
            tracing::info!("Changed    {}", dir.display());
            outcome.changed_dirs.push(dir.to_path_buf());
            outcome.any_changed = true;
        }
        None => {
            tracing::info!("New        {}", dir.display());
            outcome.changed_dirs.push(dir.to_path_buf());
            outcome.any_changed = true;
        }
    }

    // --- 5. Update cache entry ----------------------------------------------
    cache.set(dir, CacheEntry { shallow_fp, deep_fp: deep_fp.clone() });

    Ok(deep_fp)
}

/// List the direct subdirectories of `dir`, sorted for a deterministic walk
/// order (required so deep_fp is stable across runs on different OSes).
///
/// Uses plain `read_dir` — no gitignore filtering here.  The fingerprint walk
/// is purely structural; gitignore rule application is done in the processing
/// step that follows.  Including gitignored dirs in the fingerprint is harmless:
/// a change there at worst triggers an unnecessary reprocess (which is cheap).
fn list_subdirs(dir: &Path) -> eyre::Result<Vec<PathBuf>> {
    let mut subdirs = Vec::new();

    let read = std::fs::read_dir(dir)
        .map_err(|e| eyre!("Cannot read dir {}: {e}", dir.display()))?;

    for entry in read {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            subdirs.push(entry.path());
        }
    }

    subdirs.sort(); // must be sorted — deep_fp depends on stable child order
    Ok(subdirs)
}
