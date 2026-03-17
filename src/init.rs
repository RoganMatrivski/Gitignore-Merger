use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use color_eyre::Report;
use strum::{Display, EnumIter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, EnumIter, Display)]
#[strum(serialize_all = "lowercase")]
pub enum OutputFormat {
    Gitignore,
    Syncthing,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Verbosity log
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Dry run — print what would be written without touching the filesystem
    #[arg(short, long)]
    pub dry_run: bool,

    /// Skip writing if the output file already exists
    #[arg(long)]
    pub no_overwrite: bool,

    /// Output filename for the merged gitignore
    #[arg(short, long, default_value = ".gitignore")]
    pub name: String,

    /// Output filename for the Syncthing ignore file
    #[arg(long, default_value = ".stignore")]
    pub stignore_name: String,

    /// Which format(s) to write — pass multiple times for more than one
    /// e.g. `--format gitignore --format syncthing`
    /// Defaults to gitignore only.
    #[arg(short, long, default_values = ["gitignore"])]
    pub formats: Vec<OutputFormat>,

    /// Path to get
    pub path: Option<Vec<PathBuf>>,
}

const VERBOSE_LEVELS: &[&str] = &["info", "debug", "trace"];

macro_rules! pkg_name {
    () => {
        env!("CARGO_PKG_NAME").replace('-', "_")
    };
}

pub fn initialize() -> Result<Args, Report> {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    color_eyre::install()?;

    let args = Args::parse();

    let crate_level = args
        .verbose
        .min(VERBOSE_LEVELS.len() as u8)
        .checked_sub(1)
        .map(|i| VERBOSE_LEVELS[i as usize])
        .unwrap_or("warn");

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn"))
        .add_directive(format!("{}={}", pkg_name!(), crate_level).parse().unwrap());

    let fmt_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_level(true)
        .with_thread_ids(args.verbose > 1)
        .with_thread_names(args.verbose > 2);

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(env_filter)
        .with(ErrorLayer::default())
        .init();

    Ok(args)
}
