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

    /// Restrict an export to a single adapter id.
    #[arg(long, value_name = "ID")]
    pub adapter: Option<String>,

    /// Print the plan that a destructive operation would run, as JSON, and exit.
    /// Never executes anything: it is the same preview the interface shows.
    #[arg(long, value_name = "NATIVE_ID")]
    pub print_plan: Option<String>,

    /// Ask the release API whether a newer version exists, print the answer, and exit.
    /// Reads the network only; nothing is downloaded or installed.
    #[arg(long)]
    pub check_update: bool,

    /// Download, verify and install the newest release over this binary. Requires --yes.
    #[arg(long, requires = "yes")]
    pub update: bool,

    /// Confirm an action that would otherwise ask.
    #[arg(long)]
    pub yes: bool,

    /// Operation to plan with `--print-plan`.
    #[arg(
        long,
        value_name = "KIND",
        default_value = "remove",
        requires = "print_plan"
    )]
    pub kind: PlanKindArg,
}

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

/// Operations that `--print-plan` can describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PlanKindArg {
    Remove,
    Update,
    Reclaim,
}

impl From<PlanKindArg> for crate::domain::OperationKind {
    fn from(value: PlanKindArg) -> Self {
        match value {
            PlanKindArg::Remove => Self::Remove,
            PlanKindArg::Update => Self::Update,
            PlanKindArg::Reclaim => Self::Reclaim,
        }
    }
}
