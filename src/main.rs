mod checksum;
mod cli;
mod dir_list;
mod status;
mod update;
mod ward_file;

use cli::{Cli, Command};
use status::ChecksumPolicy;
use std::fmt as stdfmt;
use std::io::{IsTerminal, stderr};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing::{Event, Level, Subscriber, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use update::{WardOptions, ward_directory};

struct WardExitCode;

impl WardExitCode {
    /// Exit code used when the ward status is unclean (differences found).
    fn status_unclean() -> ExitCode {
        ExitCode::from(1)
    }

    /// Exit code used for other errors (I/O errors, invalid arguments, etc.).
    fn any_error() -> ExitCode {
        ExitCode::from(255)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    // Change working directory if -C was specified
    if let Some(directory) = cli.directory
        && let Err(e) = std::env::set_current_dir(&directory)
    {
        error!(
            "Failed to change directory to {}: {}",
            directory.display(),
            e
        );
        return WardExitCode::any_error();
    }

    let current_dir = PathBuf::from(".");

    let result: anyhow::Result<ExitCode> = match cli.command {
        Command::Update {
            allow_init,
            fingerprint,
            dry_run,
        } => handle_ward(current_dir.clone(), false, allow_init, fingerprint, dry_run),
        Command::Init {
            fingerprint,
            dry_run,
        } => handle_ward(current_dir.clone(), true, false, fingerprint, dry_run),
        Command::Status {
            verify,
            always_verify,
            all,
        } => handle_status(current_dir.clone(), verify, always_verify, all),
        Command::Verify {} => handle_verify(current_dir),
    };

    match result {
        Ok(exit_code) => exit_code,
        Err(err) => {
            error!("{err}");
            WardExitCode::any_error()
        }
    }
}

fn handle_ward(
    path: PathBuf,
    init: bool,
    allow_init: bool,
    fingerprint: Option<String>,
    dry_run: bool,
) -> anyhow::Result<ExitCode> {
    let options = WardOptions {
        init,
        allow_init,
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
        for ward_path in result.ward_files_updated {
            info!("  {}", ward_path.display());
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn handle_status(
    path: PathBuf,
    verify: bool,
    always_verify: bool,
    all: bool,
) -> anyhow::Result<ExitCode> {
    let policy = if always_verify {
        ChecksumPolicy::Always
    } else if verify {
        ChecksumPolicy::WhenPossiblyModified
    } else {
        ChecksumPolicy::Never
    };

    let mode = if all {
        status::StatusMode::All
    } else {
        status::StatusMode::Interesting
    };

    let result = status::compute_status(&path, policy, mode, status::StatusPurpose::Display)?;

    let has_interesting_changes = result
        .statuses
        .iter()
        .any(|c| c.status_type() != status::StatusType::Unchanged);

    if result.statuses.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    print_statuses(&result.statuses);

    if has_interesting_changes {
        println!();
        println!("Fingerprint: {}", result.fingerprint);
        info!(
            "Run 'treeward init|update --fingerprint {}' to accept these changes and update the ward.",
            result.fingerprint
        );

        Ok(WardExitCode::status_unclean())
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn handle_verify(path: PathBuf) -> anyhow::Result<ExitCode> {
    let result = status::compute_status(
        &path,
        ChecksumPolicy::Always,
        status::StatusMode::Interesting,
        status::StatusPurpose::Display,
    )?;

    if result.statuses.is_empty() {
        info!("Verification successful: No changes or corruption detected");
        return Ok(ExitCode::SUCCESS);
    }

    print_statuses(&result.statuses);

    error!(
        "Verification failed: {} change(s) detected",
        result.statuses.len()
    );
    Ok(WardExitCode::status_unclean())
}

fn print_statuses(statuses: &[status::StatusEntry]) {
    for entry in statuses {
        let status_code = match entry.status_type() {
            status::StatusType::Added => "A",
            status::StatusType::Removed => "R",
            status::StatusType::PossiblyModified => "M?",
            status::StatusType::Modified => "M",
            status::StatusType::Unchanged => ".",
        };

        println!("{} {}", status_code, entry.path());
    }
}

fn init_tracing(verbose: u8) {
    let stderr_is_terminal = stderr().is_terminal();
    let formatter = EmojiFormatter { stderr_is_terminal };

    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

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
                Level::DEBUG => write!(writer, "🔍 ")?,
                Level::INFO => write!(writer, "ℹ️ ")?,
                Level::WARN => write!(writer, "⚠️  ")?,
                Level::ERROR => write!(writer, "❌️ ")?,
                _ => {}
            }
        } else {
            match *event.metadata().level() {
                Level::DEBUG => writer.write_str("DEBUG: ")?,
                Level::INFO => writer.write_str("INFO: ")?,
                Level::WARN => writer.write_str("WARN: ")?,
                Level::ERROR => writer.write_str("ERROR: ")?,
                _ => {}
            }
        }

        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
