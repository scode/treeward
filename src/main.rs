mod checksum;
mod cli;
mod dir_list;
mod status;
mod ward;
mod ward_file;

use cli::{Cli, Command};
use status::ChecksumPolicy;
use std::fmt as stdfmt;
use std::io::{IsTerminal, stderr};
use std::path::PathBuf;
use std::process;
use tracing::{Event, Level, Subscriber, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use ward::{WardOptions, ward_directory};

fn main() {
    init_tracing();

    let cli = Cli::parse();

    let result = match cli.command {
        Command::Ward {
            path,
            init,
            fingerprint,
            dry_run,
        } => handle_ward(path, init, fingerprint, dry_run),
        Command::Status {
            path,
            verify,
            always_verify,
        } => handle_status(path, verify, always_verify),
        Command::Verify { path } => handle_verify(path),
    };

    if let Err(err) = result {
        error!("{err}");
        process::exit(1);
    }
}

fn handle_ward(
    path: PathBuf,
    init: bool,
    fingerprint: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let options = WardOptions {
        init,
        fingerprint,
        dry_run,
    };

    let result = ward_directory(&path, options)?;

    if dry_run {
        info!("DRY RUN - no files were modified");
    }

    info!("Warded {} files", result.files_warded);

    if !result.ward_files_updated.is_empty() {
        info!("Updated {} ward files:", result.ward_files_updated.len());
        let root = path.canonicalize().unwrap_or(path.clone());

        for ward_path in result.ward_files_updated {
            info!("  {}", root.join(ward_path).display());
        }
    }

    Ok(())
}

fn handle_status(path: PathBuf, verify: bool, always_verify: bool) -> anyhow::Result<()> {
    let policy = if always_verify {
        ChecksumPolicy::Always
    } else if verify {
        ChecksumPolicy::WhenPossiblyModified
    } else {
        ChecksumPolicy::Never
    };

    let result = status::compute_status(&path, policy)?;

    if result.changes.is_empty() {
        return Ok(());
    }

    print_changes(&result.changes);

    println!();
    println!("Fingerprint: {}", result.fingerprint);
    info!(
        "Run 'treeward ward --fingerprint {}' to accept these changes and update the ward.",
        result.fingerprint
    );

    anyhow::bail!(
        "Ward is not consistent with the filesystem ({} changes detected)",
        result.changes.len()
    );
}

fn handle_verify(path: PathBuf) -> anyhow::Result<()> {
    let result = status::compute_status(&path, ChecksumPolicy::Always)?;

    if result.changes.is_empty() {
        info!("Verification successful: No changes or corruption detected");
        return Ok(());
    }

    print_changes(&result.changes);

    anyhow::bail!(
        "Verification failed: {} changes detected",
        result.changes.len()
    );
}

fn print_changes(changes: &[status::Change]) {
    for change in changes {
        let status_code = match change.change_type {
            status::ChangeType::Added => "A",
            status::ChangeType::Removed => "R",
            status::ChangeType::PossiblyModified => "M?",
            status::ChangeType::Modified => "M",
        };

        println!("{} {}", status_code, change.path.display());
    }
}

fn init_tracing() {
    let stderr_is_terminal = stderr().is_terminal();
    let formatter = EmojiFormatter { stderr_is_terminal };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    let fmt_layer = tracing_fmt::layer()
        .event_format(formatter)
        .with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}

struct EmojiFormatter {
    stderr_is_terminal: bool,
}

impl<S, N> FormatEvent<S, N> for EmojiFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> stdfmt::Result {
        if self.stderr_is_terminal {
            match *event.metadata().level() {
                Level::WARN => write!(writer, "⚠️  ")?,
                Level::ERROR => write!(writer, "❌️ ")?,
                _ => {}
            }
        } else {
            match *event.metadata().level() {
                Level::WARN => writer.write_str("WARN: ")?,
                Level::ERROR => writer.write_str("ERROR: ")?,
                _ => {}
            }
        }

        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
