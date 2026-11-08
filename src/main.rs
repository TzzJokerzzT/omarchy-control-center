//! Omarchy Control Center — inventory and controlled lifecycle operations for
//! software installed on Omarchy / Arch Linux.

mod adapters;
mod app;
mod application;
mod cli;
mod domain;
mod infrastructure;
mod term;
mod ui;

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;

use crate::cli::Args;
use crate::infrastructure::config::Config;
use crate::infrastructure::paths::AppPaths;
use crate::infrastructure::system::{SystemInfo, guard_not_root};
use crate::application::{AdapterRegistry, operations};
use crate::infrastructure::process::SystemRunner;

fn main() -> ExitCode {
    let args = Args::parse_args();
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Args) -> anyhow::Result<ExitCode> {
    let paths =
        AppPaths::resolve(args.config.clone()).context("cannot resolve the user directories")?;
    paths.ensure().map_err(|error| anyhow::anyhow!("{error}"))?;
    infrastructure::logging::init(&paths.log_file, args.log_level.as_deref())
        .context("cannot open the diagnostic log")?;

    let system = SystemInfo::detect();
    tracing::info!(
        system = %system.display_name(),
        arch_like = system.is_arch_like,
        omarchy = system.is_omarchy,
        euid = ?system.euid,
        read_only_requested = args.read_only,
        "starting"
    );

    // The interface itself must never run privileged.
    if let Err(refusal) = guard_not_root(system.euid) {
        tracing::error!("{refusal}");
        eprintln!("{refusal}");
        return Ok(ExitCode::from(2));
    }

    let mut notices = Vec::new();
    let config = match Config::load(&paths.config_file) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "configuration");
            notices.push(format!("{error} — using defaults"));
            Config::default()
        }
    };
    if !system.is_arch_like {
        notices.push(format!(
            "{} is not Arch-based: pacman is expected to be missing and most adapters will report unavailable",
            system.display_name()
        ));
    }

    let read_only = args.read_only || config.read_only;

    let runner: Arc<dyn infrastructure::process::CommandRunner> = match &args.extra_search_path {
        Some(dir) => Arc::new(SystemRunner::with_extra_path(dir)),
        None => Arc::new(SystemRunner::new()),
    };
    for extra in &config.extra_search_paths {
        tracing::info!(path = %extra.display(), "configured extra search path");
    }
    let registry =
        adapters::build_registry(Arc::clone(&runner)).context("cannot build the adapter registry")?;

    if args.print_inventory {
        return print_inventory(&registry, args.adapter.as_deref()).map(|()| ExitCode::SUCCESS);
    }
    if let Some(target) = &args.print_plan {
        return print_plan(
            &registry,
            target,
            args.adapter.as_deref(),
            args.kind.into(),
            read_only,
        )
        .map(|()| ExitCode::SUCCESS);
    }

    app::run(app::Bootstrap {
        registry: Arc::new(registry),
        system,
        read_only,
        notices,
    })
    .context("interface failure")?;

    Ok(ExitCode::SUCCESS)
}

/// Prints exactly the plan a destructive operation would run, and exits without
/// executing anything. The interface shows this same value.
fn print_plan(
    registry: &AdapterRegistry,
    target: &str,
    only_adapter: Option<&str>,
    kind: crate::domain::OperationKind,
    read_only: bool,
) -> anyhow::Result<()> {
    #[derive(serde::Serialize)]
    struct PlanExport<'a> {
        fingerprint: &'a str,
        executable: bool,
        plan: &'a crate::domain::OperationPlan,
    }

    let snapshot = registry.load_all();
    let component = snapshot
        .components
        .iter()
        .find(|component| component.id.native_id == target)
        .filter(|component| only_adapter.is_none_or(|wanted| component.id.adapter == wanted))
        .ok_or_else(|| anyhow::anyhow!("{target} is not installed according to any adapter"))?;

    match operations::prepare(registry, component, kind, read_only) {
        Ok(pending) => {
            let export = PlanExport {
                fingerprint: pending.fingerprint(),
                executable: pending.plan.is_executable(),
                plan: &pending.plan,
            };
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, &export)
                .context("cannot serialize the plan")?;
            handle.write_all(b"\n")?;
            Ok(())
        }
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

/// Headless inventory export. Never touches the terminal and never mutates
/// anything: it only lists what the adapters report.
fn print_inventory(registry: &AdapterRegistry, only_adapter: Option<&str>) -> anyhow::Result<()> {
    #[derive(serde::Serialize)]
    struct Export<'a> {
        adapters: Vec<&'a crate::domain::AdapterStatus>,
        components: Vec<&'a crate::domain::Component>,
        failures: Vec<&'a crate::application::AdapterFailure>,
        duplicates: Vec<&'a crate::application::DuplicateIdentity>,
    }

    let snapshot = registry.load_all();
    let export = Export {
        adapters: snapshot
            .statuses
            .iter()
            .filter(|status| only_adapter.is_none_or(|wanted| status.id == wanted))
            .collect(),
        components: snapshot
            .components
            .iter()
            .filter(|component| {
                only_adapter.is_none_or(|wanted| component.id.adapter == wanted)
            })
            .collect(),
        failures: snapshot.failures.iter().collect(),
        duplicates: snapshot.duplicates.iter().collect(),
    };

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer_pretty(&mut handle, &export).context("cannot serialize the inventory")?;
    handle.write_all(b"\n")?;
    Ok(())
}
