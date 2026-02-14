//! Command-line interface schema for treeward.
//!
//! Defines clap structs/enums for global flags and subcommands.
//! Long-form command text is sourced from `help_text`.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

mod help_text;

/// Explicit logging level for CLI output.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// File integrity tool for checksumming and verifying trees
#[derive(Parser, Debug)]
#[command(
    name = "treeward",
    about,
    long_about = help_text::ROOT_LONG_ABOUT,
    disable_version_flag = true
)]
pub struct Cli {
    /// Change to directory before operating
    #[arg(short = 'C', value_name = "DIRECTORY", global = true)]
    pub directory: Option<PathBuf>,

    /// Increase verbosity (-v for info, -vv for debug).
    /// Takes precedence over RUST_LOG.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Set log level explicitly (error, warn, info, debug, trace).
    /// Takes precedence over RUST_LOG.
    #[arg(
        long = "log-level",
        value_enum,
        value_name = "LEVEL",
        conflicts_with = "verbose",
        global = true
    )]
    pub log_level: Option<LogLevel>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Update ward files with current state
    #[command(long_about = help_text::UPDATE_LONG_ABOUT)]
    Update {
        /// Allow initialization if ward files are missing
        #[arg(long)]
        allow_init: bool,

        /// Only proceed if changes match this fingerprint from status.
        /// When using this flag, ensure --verify/--always-verify flags match
        /// those used with the status command that produced the fingerprint.
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: Option<String>,

        /// Preview changes without writing ward files
        #[arg(long)]
        dry_run: bool,

        /// Verify checksums for files whose metadata changed
        #[arg(long)]
        verify: bool,

        /// Always verify checksums for all files
        #[arg(long, conflicts_with = "verify")]
        always_verify: bool,
    },

    /// Initialize ward files in a directory
    #[command(long_about = help_text::INIT_LONG_ABOUT)]
    Init {
        /// Only proceed if changes match this fingerprint from status.
        /// When using this flag, ensure --verify/--always-verify flags match
        /// those used with the status command that produced the fingerprint.
        #[arg(long, value_name = "FINGERPRINT")]
        fingerprint: Option<String>,

        /// Preview changes without writing ward files
        #[arg(long)]
        dry_run: bool,

        /// Verify checksums for files whose metadata changed
        #[arg(long)]
        verify: bool,

        /// Always verify checksums for all files
        #[arg(long, conflicts_with = "verify")]
        always_verify: bool,
    },

    /// Show status of files (added, removed, modified)
    #[command(long_about = help_text::STATUS_LONG_ABOUT)]
    Status {
        /// Verify checksums for files whose metadata changed
        #[arg(long)]
        verify: bool,

        /// Always verify checksums for all files (detect silent corruption)
        #[arg(long, conflicts_with = "verify")]
        always_verify: bool,

        /// Show all files, including unchanged ones
        #[arg(long)]
        all: bool,

        /// Show detailed diff of what changed for each entry (implies --verify)
        #[arg(long)]
        diff: bool,
    },

    /// Verify consistency of the ward, exit with success if no inconsistency.
    #[command(long_about = help_text::VERIFY_LONG_ABOUT)]
    Verify {},
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
