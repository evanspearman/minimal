//! `onboarding` — what a first run puts in front of the user: how much
//! of the machine the VM gets, then which loadout it comes up with, and
//! the answers on stdout for whatever drives the rest of setup.
//!
//! [`portrait`] holds the art and the loadout data, [`strip`] lays the
//! portraits out, [`detail`] sets the copy under them, [`resources`]
//! models the VM's share of the host, [`tui`] runs the two screens, and
//! this module is the CLI around them.

use std::io::{self, IsTerminal as _, Write as _};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

mod detail;
mod portrait;
mod resources;
mod strip;
mod tui;

use portrait::{Loadout, Size};

/// Exit status when the user backs out of the picker, following the
/// shell's 128 + SIGINT convention.
const CANCELED: u8 = 130;

#[derive(Parser)]
#[command(name = "onboarding", version = version::VERSION, long_version = version::LONG_VERSION)]
#[command(about = "Set up a session: VM resources, then a loadout")]
#[command(
    long_about = "Set up a session: VM resources, then a loadout.\n\nWith no subcommand this opens \
                  the picker — first the VM's cores and memory, then the loadout portraits — and \
                  writes the result to stdout as `vcpus=`, `ram_mib=`, and `loadout=` lines."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List the loadout slugs, one per line.
    List,
    /// Print one loadout's portrait.
    Show {
        #[arg(value_enum)]
        loadout: Loadout,

        /// Portrait size. Defaults to the largest that fits the terminal.
        #[arg(long, value_enum)]
        size: Option<Size>,
    },
}

fn main() -> Result<ExitCode> {
    match Cli::parse().command {
        Some(Command::List) => list().map(|()| ExitCode::SUCCESS),
        Some(Command::Show { loadout, size }) => {
            print_portrait(loadout, size.unwrap_or_else(fitting_size)).map(|()| ExitCode::SUCCESS)
        }
        None => pick(),
    }
}

/// The largest portrait the terminal can hold, small when the width is
/// unknowable (piped output, no terminal at all).
fn fitting_size() -> Size {
    crossterm::terminal::size().map_or(Size::Small, |(columns, _)| Size::fitting(columns))
}

fn list() -> Result<()> {
    let mut out = io::stdout().lock();
    Loadout::ALL
        .iter()
        .try_for_each(|loadout| writeln!(out, "{}", loadout.slug()))
        .context("writing the loadout list")
}

/// Portraits carry their own line terminators and end each line with a
/// color reset, so they go out verbatim.
fn print_portrait(loadout: Loadout, size: Size) -> Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(loadout.portrait(size).as_bytes())
        .with_context(|| format!("writing the {} portrait", loadout.slug()))
}

/// Run the picker, then leave the result on the terminal the user is
/// back on: the portrait, then the settings as `key=value` lines for
/// whatever reads them.
fn pick() -> Result<ExitCode> {
    anyhow::ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "the picker needs a terminal; use `onboarding list` or \
         `onboarding show <LOADOUT>` when running unattended"
    );

    // Quitting part-way is not an error: say nothing and let the exit
    // status carry it.
    let Some(choice) = tui::run()? else {
        return Ok(ExitCode::from(CANCELED));
    };

    print_portrait(choice.loadout, fitting_size())?;
    let mut out = io::stdout().lock();
    writeln!(out, "vcpus={}", choice.allocation.vcpus)
        .and_then(|()| writeln!(out, "ram_mib={}", choice.allocation.ram_mib))
        .and_then(|()| writeln!(out, "loadout={}", choice.loadout.slug()))
        .context("writing the selection")?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_loadout_is_reachable_by_slug() {
        for loadout in Loadout::ALL {
            let cli = Cli::try_parse_from(["onboarding", "show", loadout.slug()])
                .expect("slugs are the accepted command-line values");
            assert!(matches!(
                cli.command,
                Some(Command::Show { loadout: parsed, .. }) if parsed == loadout
            ));
        }
    }

    #[test]
    fn size_can_be_forced_past_the_terminal_fit() {
        let cli = Cli::try_parse_from(["onboarding", "show", "tinkerer", "--size", "large"])
            .expect("--size takes a portrait size");
        assert!(matches!(
            cli.command,
            Some(Command::Show {
                size: Some(Size::Large),
                ..
            })
        ));
    }
}
