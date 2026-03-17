use std::path::{Path, PathBuf};

use color_eyre::Report;
use eyre::ContextCompat;

mod init;

// Avoid musl's default allocator due to lackluster performance
// https://nickb.dev/blog/default-musl-allocator-considered-harmful-to-performance
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tracing::instrument]
fn main() -> Result<(), Report> {
    let args = init::initialize()?;

    let paths = if let Some(p) = args.path {
        p
    } else {
        vec![PathBuf::from(".")]
    };

    for p in paths {
        let paths = gitignore_getter(&p)?;
        let gitignore_contents = paths
            .iter()
            .map(|x| gitignore_read(x, &p))
            .collect::<eyre::Result<Vec<_>>>()?
            .join("\n")
            + "\n";

        let output_file = p.join(&args.name);

        println!("Will write to \"{}\":", output_file.to_string_lossy());
        println!("{gitignore_contents}");

        if args.dry_run {
            tracing::debug!("Dry run specified. Skipping writing to file...");
            continue;
        }

        if args.no_overwrite && std::fs::exists(&output_file).is_ok() {
            tracing::warn!(
                "Output file \"{}\" already exists. Skipping writing to file...",
                output_file.to_string_lossy()
            );
            continue;
        }

        std::fs::write(output_file, gitignore_contents)?;
    }

    Ok(())
}

fn gitignore_read(file: impl AsRef<Path>, rootdir: impl AsRef<Path>) -> eyre::Result<String> {
    let filepath = file.as_ref();
    let rootdir = rootdir.as_ref();
    let filestr = std::fs::read_to_string(filepath)?;

    let filedir = filepath
        .parent()
        .context("file should have a parent directory")?;

    let relative_path = filedir.strip_prefix(rootdir).unwrap_or(&filedir);

    let filelines = filestr
        .lines()
        .filter(|x| !x.starts_with("#"))
        .map(|l| {
            let stripped = l.trim_start_matches('/');

            relative_path.join(stripped)
        })
        .collect::<Vec<_>>();

    let joined = filelines
        .iter()
        .map(|x| x.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(joined)
}

fn gitignore_getter(srcdir: &Path) -> eyre::Result<Vec<std::path::PathBuf>> {
    use std::ffi::OsStr;
    use walkdir::WalkDir;

    let mut paths = vec![];

    for entry in WalkDir::new(srcdir).min_depth(2) {
        let e = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::error!("Error walking directory: {:?}", e);
                continue;
            }
        };

        if e.file_name() == OsStr::new(".gitignore") {
            paths.push(e.into_path());
        }
    }

    Ok(paths)
}
