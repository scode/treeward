mod checksum;
mod cli;
mod dir_list;
mod status;
mod ward;
mod ward_file;

use cli::{Cli, Command};
use status::ChecksumPolicy;
use std::path::PathBuf;
use std::process;
use ward::{WardOptions, ward_directory};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Ward {
            path,
            init,
            fingerprint,
            dry_run,
        } => handle_ward(path, init, fingerprint, dry_run),
        Command::Status { path, verify } => handle_status(path, verify),
        Command::Verify { path } => handle_verify(path),
    };

    if let Err(err) = result {
        eprintln!("Error: {}", err);
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
        println!("DRY RUN - no files were modified");
    }

    println!("Warded {} files", result.files_warded);

    if !result.ward_files_updated.is_empty() {
        println!("Updated {} ward files:", result.ward_files_updated.len());
        let root = path.canonicalize().unwrap_or(path.clone());

        for ward_path in result.ward_files_updated {
            println!("  {}", root.join(ward_path).display());
        }
    }

    Ok(())
}

fn handle_status(path: PathBuf, verify: bool) -> anyhow::Result<()> {
    let policy = if verify {
        ChecksumPolicy::Always
    } else {
        ChecksumPolicy::WhenPossiblyModified
    };

    let result = status::compute_status(&path, policy)?;

    if result.changes.is_empty() {
        println!("No changes detected");
        return Ok(());
    }

    print_changes(&result.changes);

    println!();
    println!(
        "Run 'treeward ward --fingerprint {}' to accept these changes and update the ward.",
        result.fingerprint
    );

    Ok(())
}

fn handle_verify(path: PathBuf) -> anyhow::Result<()> {
    let result = status::compute_status(&path, ChecksumPolicy::Always)?;

    if result.changes.is_empty() {
        println!("Verification successful: No changes or corruption detected");
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
