mod conflict;
mod games;
mod model;
mod scan;
#[cfg(test)]
mod testutil;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use games::Game;
use model::{Conflict, Severity};

/// Scan a game mod folder and report conflicts before the game crashes.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Folder containing the mods.
    path: PathBuf,

    /// Which game the folder belongs to. Detected automatically when omitted.
    #[arg(short, long, value_enum)]
    game: Option<Game>,

    /// Browse the results in an interactive terminal UI.
    #[arg(short, long)]
    tui: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(clean) if clean => ExitCode::SUCCESS,
        // Conflicts found: non-zero so this is usable in a pre-launch script.
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Returns whether the folder is clean.
fn run() -> Result<bool> {
    let cli = Cli::parse();

    let raw = scan::scan_dir(&cli.path)?;
    let game = match cli.game {
        Some(g) => g,
        None => Game::detect(&raw)?,
    };

    let mods = game.parse_mods(&raw);
    let conflicts = conflict::detect(&mods, game.detect_options());

    if cli.tui {
        tui::run(conflicts.clone())?;
    } else {
        print_report(game, mods.len(), &conflicts);
    }
    Ok(conflicts.is_empty())
}

fn print_report(game: Game, mod_count: usize, conflicts: &[Conflict]) {
    println!("{game}: scanned {mod_count} mods");

    if conflicts.is_empty() {
        println!("no conflicts found");
        return;
    }

    let critical = conflicts
        .iter()
        .filter(|c| c.severity() == Severity::Critical)
        .count();
    println!(
        "{} conflicts ({critical} critical)\n",
        conflicts.len()
    );

    for c in conflicts {
        println!("[{}] {}", c.severity(), c.title());
    }
    println!("\nRun with --tui for details.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{info_json, write_zip_mod};

    /// End to end over a real folder of real zips: scan, parse, detect.
    #[test]
    fn finds_the_planted_conflicts_in_a_factorio_folder() {
        let dir = tempfile::tempdir().unwrap();

        // Requires a version of `base` that is not installed.
        write_zip_mod(
            dir.path(),
            "alpha_1.0.0.zip",
            &[(
                "alpha_1.0.0/info.json",
                &info_json("alpha", "1.0.0", &["base >= 2.0.0", "! gamma"]),
            )],
        );
        write_zip_mod(
            dir.path(),
            "base_1.1.0.zip",
            &[("base_1.1.0/info.json", &info_json("base", "1.1.0", &[]))],
        );
        // Declared incompatible by alpha, and installed.
        write_zip_mod(
            dir.path(),
            "gamma_1.0.0.zip",
            &[("gamma_1.0.0/info.json", &info_json("gamma", "1.0.0", &[]))],
        );
        // Same internal path as gamma — must NOT be reported for Factorio.
        write_zip_mod(
            dir.path(),
            "delta_1.0.0.zip",
            &[
                ("delta_1.0.0/info.json", &info_json("delta", "1.0.0", &[])),
                ("delta_1.0.0/data.lua", "-- d"),
            ],
        );

        let raw = scan::scan_dir(dir.path()).unwrap();
        let game = Game::detect(&raw).unwrap();
        let mods = game.parse_mods(&raw);
        let conflicts = conflict::detect(&mods, game.detect_options());

        assert_eq!(game, Game::Factorio);
        assert_eq!(mods.len(), 4);
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::VersionMismatch { .. })));
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::Incompatible { .. })));
    }

    #[test]
    fn a_clean_folder_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(
            dir.path(),
            "base_1.1.0.zip",
            &[("base_1.1.0/info.json", &info_json("base", "1.1.0", &[]))],
        );
        write_zip_mod(
            dir.path(),
            "alpha_1.0.0.zip",
            &[(
                "alpha_1.0.0/info.json",
                &info_json("alpha", "1.0.0", &["base >= 1.0.0"]),
            )],
        );

        let raw = scan::scan_dir(dir.path()).unwrap();
        let game = Game::detect(&raw).unwrap();
        let mods = game.parse_mods(&raw);

        assert!(conflict::detect(&mods, game.detect_options()).is_empty());
    }

    #[test]
    fn detection_fails_loudly_on_an_unrecognized_folder() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(dir.path(), "mystery.zip", &[("readme.txt", "hello")]);

        let raw = scan::scan_dir(dir.path()).unwrap();

        assert!(Game::detect(&raw).is_err());
    }
}
