use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eyre::ContextCompat;
use ignore::{DirEntry, Error, ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};

// ---------------------------------------------------------------------------
// Rule types  (unchanged from original)
// ---------------------------------------------------------------------------

/// A single gitignore rule with its path context stripped of comments/blanks.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pattern: String,
    pub negated: bool,
    pub dir_only: bool,
}

#[allow(unused)]
impl Rule {
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (negated, rest) = if let Some(s) = line.strip_prefix('!') {
            (true, s)
        } else {
            (false, line)
        };
        let (dir_only, pattern) = if rest.ends_with('/') {
            (true, rest.trim_end_matches('/').to_string())
        } else {
            (false, rest.to_string())
        };
        Some(Rule {
            pattern,
            negated,
            dir_only,
        })
    }

    pub fn to_gitignore_line(&self) -> String {
        let mut s = String::new();
        if self.negated {
            s.push('!');
        }
        s.push_str(&self.pattern);
        if self.dir_only {
            s.push('/');
        }
        s
    }
}

/// A `Rule` prefixed with its path relative to the repo root.
#[derive(Debug, Clone)]
pub struct PrefixedRule {
    pub rule: Rule,
    pub relative_dir: PathBuf,
}

impl PrefixedRule {
    pub fn to_gitignore_line(&self) -> String {
        let prefix = self.relative_dir.to_string_lossy();
        let stripped = self.rule.pattern.trim_start_matches('/');
        let prefixed_pattern = if prefix.is_empty() {
            stripped.to_string()
        } else {
            format!("{}/{}", prefix, stripped)
        };
        let mut s = String::new();
        if self.rule.negated {
            s.push('!');
        }
        if !prefix.is_empty() {
            s.push('/');
        }
        s.push_str(&prefixed_pattern);
        if self.rule.dir_only {
            s.push('/');
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Generic parallel file-collector visitor  (replaces FindVisitor/FindVisitorBuilder)
//
// Both the walker (fingerprinting pass) and find_gitignores reuse this.
// Give it a filename to match, get back a Vec<PathBuf> of every hit.
// ---------------------------------------------------------------------------

pub struct FileCollectorBuilder {
    target: &'static str,
    results: Arc<Mutex<Vec<PathBuf>>>,
}

pub struct FileCollectorVisitor {
    target: &'static str,
    results: Arc<Mutex<Vec<PathBuf>>>,
}

impl FileCollectorBuilder {
    pub fn new(target: &'static str) -> (Self, Arc<Mutex<Vec<PathBuf>>>) {
        let results = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                target,
                results: Arc::clone(&results),
            },
            results.clone(),
        )
    }
}

impl<'s> ParallelVisitorBuilder<'s> for FileCollectorBuilder {
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        Box::new(FileCollectorVisitor {
            target: self.target,
            results: Arc::clone(&self.results),
        })
    }
}

impl ParallelVisitor for FileCollectorVisitor {
    fn visit(&mut self, entry: Result<DirEntry, Error>) -> WalkState {
        if let Ok(e) = entry {
            if e.file_name() == self.target {
                self.results.lock().unwrap().push(e.into_path());
            }
        }
        WalkState::Continue
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Recursively find all `.gitignore` files under `srcdir` (min depth 2).
/// Uses the shared `FileCollectorBuilder` visitor.
pub fn find_gitignores(srcdir: &Path) -> eyre::Result<Vec<PathBuf>> {
    let (mut builder, results) = FileCollectorBuilder::new(".gitignore");

    WalkBuilder::new(srcdir)
        .min_depth(Some(2))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build_parallel()
        .visit(&mut builder);

    drop(builder);

    let paths = Arc::try_unwrap(results)
        .map_err(|_| eyre::eyre!("results Arc still has multiple owners"))?
        .into_inner()?;

    tracing::debug!(count = paths.len(), ".gitignore files found");
    Ok(paths)
}

/// Read a single `.gitignore` file and return its rules with path context.
/// Called only on cache misses — cached dirs skip this entirely.
pub fn read_gitignore(file: &Path, rootdir: &Path) -> eyre::Result<Vec<PrefixedRule>> {
    let contents = std::fs::read_to_string(file)?;

    let filedir = file
        .parent()
        .context("gitignore file should have a parent directory")?;

    let relative_dir = filedir
        .strip_prefix(rootdir)
        .unwrap_or(filedir)
        .to_path_buf();

    let rules = contents
        .lines()
        .filter_map(Rule::parse)
        .map(|rule| PrefixedRule {
            rule,
            relative_dir: relative_dir.clone(),
        })
        .collect();

    Ok(rules)
}

/// Merge a list of prefixed rules into a single `.gitignore`-format string.
pub fn merge_to_gitignore(rules: &[PrefixedRule]) -> String {
    let mut out = rules
        .iter()
        .map(|r| r.to_gitignore_line())
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}
