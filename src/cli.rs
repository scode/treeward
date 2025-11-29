use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// File integrity tool for checksumming and verifying trees
#[derive(Parser, Debug)]
#[command(
    name = "treeward",
    about,
    long_about = None,
    disable_version_flag = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Update ward files with current state
    Update {
        /// Directory to update
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Allow initialization if ward files are missing
        #[arg(long)]
        allow_init: bool,

        /// Only proceed if changes match this fingerprint from status
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: Option<String>,

        /// Preview changes without writing ward files
        #[arg(long)]
        dry_run: bool,
    },

    /// Initialize ward files in a directory
    Init {
        /// Directory to initialize
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Only proceed if changes match this fingerprint from status
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: Option<String>,

        /// Preview changes without writing ward files
        #[arg(long)]
        dry_run: bool,
    },

    /// Show status of files (added, removed, modified)
    Status {
        /// Directory to check
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Verify checksums for files whose metadata changed
        #[arg(long)]
        verify: bool,

        /// Always verify checksums for all files (detect silent corruption)
        #[arg(long, conflicts_with = "verify")]
        always_verify: bool,
    },

    /// Verify consistency of the ward, exit with success if no inconsistency.
    Verify {
        /// Directory to verify
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
