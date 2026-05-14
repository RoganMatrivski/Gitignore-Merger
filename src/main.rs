use std::path::{Path, PathBuf};

use color_eyre::Report;
use strum::IntoEnumIterator;

use init::OutputFormat;

mod cache;
mod fingerprint;
mod gitignore;
mod init;
mod syncthing;
mod walker;

#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tracing::instrument]
fn main() -> Result<(), Report> {
    let mut args = init::initialize()?;

    // If launched without a terminal (e.g. double-clicked in Explorer),
    // detect which format to use from the exe filename and override the default.
    // Works by iterating OutputFormat variants and checking if the exe stem
    // contains the variant's lowercase name — adding a new variant is enough
    // to extend this.
    if let Some(detected) = detect_format_from_exe() {
        args.formats = vec![detected];
    }

    let roots: Vec<PathBuf> = args.path.take().unwrap_or_else(|| vec![PathBuf::from(".")]);

    for root in &roots {
        process_root(root, &args)?;
    }

    Ok(())
}

fn detect_format_from_exe() -> Option<OutputFormat> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().to_lowercase();
    OutputFormat::iter().find(|fmt| stem.contains(&fmt.to_string()))
}

fn process_root(root: &Path, args: &init::Args) -> eyre::Result<()> {
    // ------------------------------------------------------------------
    // 1. Load the fingerprint cache stored at root/_dir-processor-cache.json
    // ------------------------------------------------------------------
    let mut cache = cache::Cache::load(root)?;

    // ------------------------------------------------------------------
    // 2. Walk the tree and detect what changed since last run.
    //
    //    walk_cached touches every directory but only does cheap stat
    //    calls (read_dir + metadata).  No file contents are read here.
    // ------------------------------------------------------------------
    let outcome = walker::walk_cached(root, &mut cache)?;

    // ------------------------------------------------------------------
    // 3. Persist the updated fingerprints immediately so even a later
    //    crash doesn't leave the cache stale.
    // ------------------------------------------------------------------
    cache.save()?;

    // ------------------------------------------------------------------
    // 4. Skip all output generation if nothing changed.
    //
    //    This is the main win: on a clean re-run the whole process_root
    //    returns here in milliseconds instead of re-reading every
    //    gitignore in the tree.
    // ------------------------------------------------------------------
    if !outcome.any_changed {
        println!("[{}] Nothing changed — skipping", root.display());
        return Ok(());
    }

    println!(
        "[{}] {} dir(s) changed — reprocessing",
        root.display(),
        outcome.changed_dirs.len()
    );

    for changed in &outcome.changed_dirs {
        tracing::debug!("  {}", changed.display());
    }

    // ------------------------------------------------------------------
    // 5. Something changed — run the existing gitignore processing.
    //
    //    The original logic is untouched: find all gitignores, merge
    //    rules, write output.  A future optimisation could re-read only
    //    the changed subtrees (outcome.changed_dirs), but the full merge
    //    is fast enough for most projects.
    // ------------------------------------------------------------------
    let gitignore_files = gitignore::find_gitignores(root)?;

    let rules: Vec<gitignore::PrefixedRule> = gitignore_files
        .iter()
        .map(|p| gitignore::read_gitignore(p, root))
        .collect::<eyre::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();

    for fmt in &args.formats {
        match fmt {
            OutputFormat::Gitignore => {
                let content = gitignore::merge_to_gitignore(&rules);
                let dest = root.join(&args.name);
                write_output(&dest, &content, args.dry_run, args.no_overwrite)?;
            }
            OutputFormat::Syncthing => {
                let content = syncthing::merge_to_stignore(&rules);
                let dest = root.join(&args.stignore_name);
                write_output(&dest, &content, args.dry_run, args.no_overwrite)?;
            }
        }
    }

    Ok(())
}

fn write_output(dest: &Path, content: &str, dry_run: bool, no_overwrite: bool) -> eyre::Result<()> {
    println!("Will write to \"{}\":", dest.to_string_lossy());
    println!("{content}");

    if dry_run {
        tracing::debug!("Dry run specified. Skipping write to file...");
        return Ok(());
    }

    if no_overwrite && dest.exists() {
        tracing::warn!(
            "Output file \"{}\" already exists. Skipping write...",
            dest.to_string_lossy()
        );
        return Ok(());
    }

    std::fs::write(dest, content)?;
    Ok(())
}
