use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use clap::Parser;

use rotatelog::duration::parse_duration;
use rotatelog::rotate::{needs_rotation, rotate, RotateConfig};
use rotatelog::size::{format_size, parse_size};

/// Rotate a single log file by size or age — logrotate's core job, usable
/// standalone on any file without syslog integration or a config file.
#[derive(Parser, Debug)]
#[command(name = "rotatelog", version, about)]
struct Cli {
    /// The file to rotate (e.g. app.log)
    file: PathBuf,

    /// Rotate once the file reaches this size (e.g. 10M, 512K, 2048)
    #[arg(long, value_name = "SIZE")]
    max_size: Option<String>,

    /// Rotate once the file is at least this old (e.g. 1d, 12h, 30m)
    #[arg(long, value_name = "DURATION")]
    max_age: Option<String>,

    /// How many rotated copies to keep (oldest pruned beyond this)
    #[arg(long, default_value_t = 5)]
    max_count: u32,

    /// gzip each rotated copy
    #[arg(long)]
    compress: bool,

    /// Rotate immediately regardless of --max-size/--max-age
    #[arg(long)]
    force: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.max_count == 0 {
        bail!("--max-count must be at least 1");
    }

    let max_size = cli.max_size.as_deref().map(parse_size).transpose()?;
    let max_age = cli.max_age.as_deref().map(parse_duration).transpose()?;

    if !cli.force && max_size.is_none() && max_age.is_none() {
        bail!("specify --max-size and/or --max-age, or pass --force to rotate unconditionally");
    }

    if !cli.file.exists() {
        bail!("{} does not exist", cli.file.display());
    }

    let config = RotateConfig {
        max_size,
        max_age,
        max_count: cli.max_count,
        compress: cli.compress,
    };

    let should_rotate = if cli.force {
        true
    } else {
        needs_rotation(&cli.file, &config, SystemTime::now())
            .with_context(|| format!("checking whether {} needs rotation", cli.file.display()))?
    };

    if !should_rotate {
        let size = std::fs::metadata(&cli.file)?.len();
        println!(
            "rotatelog: {} is {} — below configured thresholds, not rotating",
            cli.file.display(),
            format_size(size)
        );
        return Ok(());
    }

    let outcome =
        rotate(&cli.file, &config).with_context(|| format!("rotating {}", cli.file.display()))?;

    println!(
        "rotatelog: rotated {} ({}) -> {}",
        cli.file.display(),
        format_size(outcome.rotated_size),
        outcome.new_entry_name
    );
    for (from, to) in &outcome.shifted {
        println!("rotatelog:   shifted {from} -> {to}");
    }
    for pruned in &outcome.pruned {
        println!(
            "rotatelog:   pruned {pruned} (past --max-count {})",
            cli.max_count
        );
    }

    Ok(())
}
