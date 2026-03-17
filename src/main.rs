use std::path::{Path, PathBuf};

use color_eyre::Report;
use strum::IntoEnumIterator;

use init::OutputFormat;

mod gitignore;
mod init;
mod syncthing;

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
