use std::path::{Path, PathBuf};

use eyre::ContextCompat;

/// A single gitignore rule with its path context stripped of comments/blanks.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The raw pattern text (e.g. `*.log`, `!keep.log`, `/build`)
    pub pattern: String,
    /// True when the line starts with `!` (negation rule)
    pub negated: bool,
    /// True when the pattern explicitly targets only directories (trailing `/`)
    pub dir_only: bool,
}

impl Rule {
    /// Parse a single non-comment, non-blank gitignore line into a `Rule`.
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

    /// Reconstruct the canonical gitignore line for this rule.
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

/// A `Rule` that has been prefixed with its path relative to the repo root.
#[derive(Debug, Clone)]
pub struct PrefixedRule {
    pub rule: Rule,
    /// Relative directory that owns the originating `.gitignore`, e.g. `src/foo`
    pub relative_dir: PathBuf,
}

impl PrefixedRule {
    /// Produce the final gitignore-compatible line with path prefix applied.
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
        // Always anchor with `/` when we have a concrete path prefix
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
// Public API
// ---------------------------------------------------------------------------

/// Recursively find all `.gitignore` files under `srcdir`, skipping the root
/// level (depth 1) so the output file is never included in its own source.
pub fn find_gitignores(srcdir: &Path) -> eyre::Result<Vec<PathBuf>> {
    use std::ffi::OsStr;
    use walkdir::WalkDir;

    let mut paths = Vec::new();

    for entry in WalkDir::new(srcdir).min_depth(2) {
        let e = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::error!("Error walking directory: {:?}", err);
                continue;
            }
        };

        if e.file_name() == OsStr::new(".gitignore") {
            paths.push(e.into_path());
        }
    }

    Ok(paths)
}

/// Read a single `.gitignore` file and return its rules with path context.
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
