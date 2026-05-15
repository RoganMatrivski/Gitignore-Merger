use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ignore::WalkBuilder;
use ignore::{DirEntry, Error, ParallelVisitor, ParallelVisitorBuilder, WalkState};

use crate::cache::{Cache, CacheEntry, CachedRule};
use crate::fingerprint::{compute_deep_fp, fingerprint_dir};
use crate::gitignore::{read_gitignore, FileCollectorBuilder, PrefixedRule};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct WalkOutcome {
    pub any_changed: bool,
    pub changed_dirs: Vec<PathBuf>,
    /// All gitignore rules across the whole tree — either from cache (fast)
    /// or freshly read from disk (on cache miss). Ready to pass to merge_to_gitignore.
    pub rules: Vec<PrefixedRule>,
}

/// Walk `root` in two phases:
///   Phase 1 (parallel) — fingerprint every dir + collect .gitignore paths
///   Phase 2 (sequential) — bottom-up deep_fp, cache diff, rule loading
pub fn walk_cached(root: &Path, cache: &mut Cache) -> eyre::Result<WalkOutcome> {
    let (shallow_map, gitignore_paths) = collect_parallel(root)?;
    let outcome = compute_and_diff(root, shallow_map, gitignore_paths, cache)?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Phase 1 — parallel: fingerprint dirs + find .gitignore files in one pass
// ---------------------------------------------------------------------------

fn collect_parallel(
    root: &Path,
) -> eyre::Result<(HashMap<PathBuf, String>, HashMap<PathBuf, PathBuf>)> {
    // shared state written to by worker threads
    let shallow: Arc<Mutex<HashMap<PathBuf, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let ignores: Arc<Mutex<HashMap<PathBuf, PathBuf>>> = Arc::new(Mutex::new(HashMap::new()));
    // ignores maps: parent_dir → .gitignore path

    WalkBuilder::new(root)
        .hidden(false)
        .min_depth(Some(2))
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build_parallel()
        .visit(&mut CombinedVisitorBuilder {
            shallow: Arc::clone(&shallow),
            ignores: Arc::clone(&ignores),
        });

    let shallow_map = Arc::try_unwrap(shallow)
        .expect("Arc still borrowed after build_parallel")
        .into_inner()?;

    let gitignore_paths = Arc::try_unwrap(ignores)
        .expect("Arc still borrowed after build_parallel")
        .into_inner()?;

    tracing::debug!(
        dirs = shallow_map.len(),
        gitignores = gitignore_paths.len(),
        "Phase 1 complete"
    );
    Ok((shallow_map, gitignore_paths))
}

// --- Combined visitor -------------------------------------------------------
// Fingerprints dirs AND collects .gitignore paths in a single parallel pass,
// avoiding a second full tree walk.

struct CombinedVisitorBuilder {
    shallow: Arc<Mutex<HashMap<PathBuf, String>>>,
    ignores: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

struct CombinedVisitor {
    shallow: Arc<Mutex<HashMap<PathBuf, String>>>,
    ignores: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

impl<'s> ParallelVisitorBuilder<'s> for CombinedVisitorBuilder {
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        Box::new(CombinedVisitor {
            shallow: Arc::clone(&self.shallow),
            ignores: Arc::clone(&self.ignores),
        })
    }
}

impl ParallelVisitor for CombinedVisitor {
    fn visit(&mut self, entry: Result<DirEntry, Error>) -> WalkState {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Walk error: {e}");
                return WalkState::Continue;
            }
        };

        let ft = match entry.file_type() {
            Some(ft) => ft,
            None => return WalkState::Continue,
        };

        if ft.is_dir() {
            // Fingerprint this directory
            let path = entry.into_path();
            let fp = fingerprint_dir(&path).unwrap_or_else(|e| {
                tracing::warn!("Cannot fingerprint {}: {e}", path.display());
                String::new()
            });
            self.shallow.lock().unwrap().insert(path, fp);
        } else if ft.is_file() && entry.file_name() == ".gitignore" {
            // Record which dir this .gitignore belongs to
            let gitignore_path = entry.into_path();
            if let Some(parent) = gitignore_path.parent() {
                self.ignores
                    .lock()
                    .unwrap()
                    .insert(parent.to_path_buf(), gitignore_path);
            }
        }

        WalkState::Continue
    }
}

// ---------------------------------------------------------------------------
// Phase 2 — sequential: bottom-up deep_fp + cache diff + rule collection
// ---------------------------------------------------------------------------

fn compute_and_diff(
    root: &Path,
    shallow_map: HashMap<PathBuf, String>,
    gitignore_paths: HashMap<PathBuf, PathBuf>, // dir → .gitignore path
    cache: &mut Cache,
) -> eyre::Result<WalkOutcome> {
    // Build parent → sorted-children index
    let mut children_of: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for path in shallow_map.keys() {
        if let Some(parent) = path.parent() {
            children_of
                .entry(parent.to_path_buf())
                .or_default()
                .push(path.clone());
        }
    }
    for kids in children_of.values_mut() {
        kids.sort();
    }

    // Deepest paths first — guarantees children's deep_fps are ready for parents
    let mut paths: Vec<&PathBuf> = shallow_map.keys().collect();
    paths.sort_by_key(|p| Reverse(p.components().count()));

    let mut deep_map: HashMap<&PathBuf, String> = HashMap::with_capacity(paths.len());
    let mut all_rules: Vec<PrefixedRule> = Vec::new();
    let mut outcome = WalkOutcome::default();

    for path in &paths {
        let shallow_fp = &shallow_map[*path];

        let child_deep_fps: Vec<String> = children_of
            .get(*path)
            .map(|kids| {
                kids.iter()
                    .filter_map(|c| deep_map.get(c).cloned())
                    .collect()
            })
            .unwrap_or_default();

        let deep_fp = compute_deep_fp(shallow_fp, &child_deep_fps);

        // ---- Cache diff ------------------------------------------------
        let cached = cache.get(path);
        let changed = match cached {
            Some(e) if e.deep_fp == deep_fp => false,
            Some(_) => {
                tracing::info!("Changed {}", path.display());
                true
            }
            None => {
                tracing::info!("New     {}", path.display());
                true
            }
        };

        if changed {
            outcome.changed_dirs.push(path.to_path_buf());
            outcome.any_changed = true;
        }

        // ---- Gitignore rules -------------------------------------------
        // Has a .gitignore?
        let has_gitignore = gitignore_paths.contains_key(*path);

        let dir_rules: Option<Vec<PrefixedRule>> = if !changed {
            // Cache hit — reuse stored rules, skip read_gitignore entirely
            cached.and_then(|e| e.rules.as_ref()).map(|cached_rules| {
                cached_rules
                    .iter()
                    .cloned()
                    .map(CachedRule::into_prefixed)
                    .collect()
            })
        } else if has_gitignore {
            // Cache miss and there's a .gitignore — read it fresh
            let gi_path = &gitignore_paths[*path];
            match read_gitignore(gi_path, root) {
                Ok(rules) => Some(rules),
                Err(e) => {
                    tracing::warn!("Cannot read {}: {e}", gi_path.display());
                    None
                }
            }
        } else {
            // No .gitignore in this dir
            None
        };

        // Accumulate rules for the caller
        if let Some(ref rules) = dir_rules {
            all_rules.extend(rules.iter().cloned());
        }

        // ---- Update cache ----------------------------------------------
        let cached_rules = dir_rules
            .as_deref()
            .map(|rules| rules.iter().map(CachedRule::from_prefixed).collect());

        cache.set(
            path,
            CacheEntry {
                shallow_fp: shallow_fp.clone(),
                deep_fp: deep_fp.clone(),
                rules: cached_rules,
            },
        );

        deep_map.insert(path, deep_fp);
    }

    outcome.rules = all_rules;
    Ok(outcome)
}
