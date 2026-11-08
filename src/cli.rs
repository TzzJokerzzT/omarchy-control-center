//! Command line surface.

use std::path::PathBuf;

use clap::Parser;

/// Inventory and controlled lifecycle operations for software installed on
/// Omarchy / Arch Linux.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "omarchy-control-center",
    version,
    about,
    long_about = "Inventory every installed component (pacman, AUR, optional flatpak), inspect it, \
                  and run destructive operations only after an explicit, itemised confirmation."
)]
pub struct Args {
    /// Explore the inventory without exposing any destructive action.
    #[arg(long)]
    pub read_only: bool,

    /// Log level for the diagnostic log file (error, warn, info, debug, trace).
    #[arg(long, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Configuration file to use instead of the default XDG location.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Additional directory prepended to the executable search path.
    #[arg(long, value_name = "DIR")]
    pub extra_search_path: Option<PathBuf>,

    /// Print the inventory as JSON and exit (headless export, useful over ssh).
    #[arg(long)]
    pub print_inventory: bool,

    /// Restrict `--print-inventory` to a single adapter id.
    #[arg(long, value_name = "ID", requires = "print_inventory")]
    pub adapter: Option<String>,
}

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
