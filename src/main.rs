//! ModConflict command-line interface. The engine lives in the library crate;
//! this only turns flags into a call to `analyze::run` and prints the report.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use modconflict::{analyze, baseline, ignore, manager, model, profile, report, tui};

/// Scan a game mod folder and report conflicts before the game crashes.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Folder containing the mods.
    path: PathBuf,

    /// Which game the folder belongs to. Detected automatically when omitted.
    #[arg(short, long)]
    game: Option<String>,

    /// Browse the results in an interactive terminal UI.
    #[arg(short, long, conflicts_with = "json")]
    tui: bool,

    /// Print the report as JSON.
    #[arg(short, long)]
    json: bool,

    /// Load order file to use, overriding the one the profile looks for.
    #[arg(short, long, value_name = "FILE")]
    load_order: Option<PathBuf>,

    /// Directory of extra game profiles (.toml), which override the built-ins.
    #[arg(long, value_name = "DIR")]
    profiles: Option<PathBuf>,

    /// Ignore-rules file. Defaults to `modconflict.toml` in the mod folder when
    /// present. Every suppressed finding is still counted in the report.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// A prior `--json` report to treat as accepted, so only conflicts new
    /// since then are shown. Create one with `--json > baseline.json`.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Which mod manager governs this folder. Detected automatically when
    /// omitted; `none` ignores one that is there.
    #[arg(short, long, value_enum)]
    manager: Option<manager::Manager>,

    /// Skip the record-level pass. Parsing every plugin is the slow part of a
    /// large Bethesda load order.
    #[arg(long)]
    no_records: bool,

    /// Skip hashing overlapping files. Without it, two mods shipping the same
    /// bytes cannot be told apart from two mods shipping different ones.
    #[arg(long)]
    no_hash: bool,

    /// List the games this build knows about, and exit.
    #[arg(long)]
    list_games: bool,

    /// Report only conflicts at this severity or above.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    severity: Option<model::Severity>,

    /// Report only conflicts involving a mod whose id contains this text
    /// (case-insensitive). Repeatable; a conflict matching any is kept.
    #[arg(long = "mod", value_name = "NAME")]
    mods: Vec<String>,

    /// Report only conflicts of this kind. Repeatable.
    #[arg(long, value_enum, value_name = "KIND")]
    kind: Vec<model::ConflictKind>,

    /// Exit non-zero when a conflict at this severity or above is found.
    /// Defaults to `warning` (config `fail-on`, else built-in) — `info`
    /// findings never fail the exit code.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    fail_on: Option<model::Severity>,
}

/// Drop the conflicts the filters exclude. No filter given means keep all.
fn filter_conflicts(cli: &Cli, conflicts: &mut Vec<model::Conflict>) {
    conflicts.retain(|c| {
        cli.severity.is_none_or(|min| c.severity() >= min)
            && (cli.mods.is_empty()
                || cli.mods.iter().any(|q| {
                    let q = q.to_ascii_lowercase();
                    c.mods().iter().any(|m| m.to_ascii_lowercase().contains(&q))
                }))
            && (cli.kind.is_empty() || cli.kind.contains(&c.kind()))
    });
}

fn main() -> ExitCode {
    match run() {
        Ok(clean) if clean => ExitCode::SUCCESS,
        // Something actionable was found: non-zero so this is usable as a
        // pre-launch check.
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

    if cli.list_games {
        for p in profile::load_all(cli.profiles.as_deref())? {
            println!("{:<20} {}", p.name, p.display_name);
        }
        return Ok(true);
    }

    // One read of modconflict.toml: flag defaults and ignore rules both.
    let ignore::Config { settings, rules } =
        ignore::Config::load(&cli.path, cli.config.as_deref())?;

    // A flag on the command line always wins over the file; the file wins over
    // the built-in default. Boolean flags only ever enable, so OR is the merge.
    let game = cli.game.clone().or_else(|| settings.game.clone());
    let profiles = cli.profiles.clone().or_else(|| settings.profiles.clone());

    let baseline = match &cli.baseline {
        Some(path) => baseline::Baseline::load(path)?,
        None => baseline::Baseline::default(),
    };

    let mut analysis = analyze::run(
        &cli.path,
        &analyze::Options {
            game: game.as_deref(),
            load_order: cli.load_order.as_deref(),
            profiles_dir: profiles.as_deref(),
            manager: cli.manager.or(settings.manager),
            skip_records: cli.no_records || settings.no_records.unwrap_or(false),
            skip_hashing: cli.no_hash || settings.no_hash.unwrap_or(false),
            ignore: rules,
            baseline,
        },
    )?;

    for problem in &analysis.unreadable_plugins {
        eprintln!("warning: cannot read plugin {problem}");
    }

    filter_conflicts(&cli, &mut analysis.conflicts);

    let fail_on = cli
        .fail_on
        .or(settings.fail_on)
        .unwrap_or(model::Severity::Warning);

    let report = analysis.report();
    if cli.tui {
        tui::run(analysis.conflicts.clone())?;
    } else if cli.json {
        report::print_json(&report)?;
    } else {
        report::print_text(&report);
    }
    // Clean when nothing reaches the fail-on threshold. The filters above have
    // already narrowed the set, so `--mod X --fail-on critical` fails only on a
    // critical involving X — the exit code reflects what was reported.
    Ok(!report.conflicts.iter().any(|c| c.severity() >= fail_on))
}

#[cfg(test)]
mod tests {
    use super::*;
    use modconflict::model::Conflict;

    fn sample_conflicts() -> Vec<Conflict> {
        use modconflict::model::{Dep, DepKind};
        vec![
            // Critical, involves alpha + base.
            Conflict::MissingDep {
                mod_id: "alpha".into(),
                dep: Dep {
                    name: "base".into(),
                    req: None,
                    kind: DepKind::Required,
                    syntax: Default::default(),
                },
            },
            // Warning, involves beta + gamma.
            Conflict::FileOverlap {
                path: "assets/stone.png".into(),
                mods: vec!["beta".into(), "gamma".into()],
                winner: None,
                identical: false,
            },
            // Info, involves beta + delta.
            Conflict::RedundantMod {
                mod_id: "beta".into(),
                duplicate_of: "delta".into(),
                files: 3,
            },
        ]
    }

    fn parse_cli(args: &[&str]) -> Cli {
        let mut full = vec!["modconflict", "some/path"];
        full.extend_from_slice(args);
        Cli::parse_from(full)
    }

    #[test]
    fn no_filter_keeps_every_conflict() {
        let mut conflicts = sample_conflicts();
        filter_conflicts(&parse_cli(&[]), &mut conflicts);
        assert_eq!(conflicts.len(), 3);
    }

    #[test]
    fn severity_filter_drops_below_the_floor() {
        let mut conflicts = sample_conflicts();
        filter_conflicts(&parse_cli(&["--severity", "warning"]), &mut conflicts);
        // The info-level redundant mod is gone; critical and warning remain.
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts
            .iter()
            .all(|c| c.severity() > model::Severity::Info));
    }

    #[test]
    fn mod_filter_matches_a_substring_case_insensitively() {
        let mut conflicts = sample_conflicts();
        filter_conflicts(&parse_cli(&["--mod", "GAMM"]), &mut conflicts);
        // Only the overlap names gamma.
        assert_eq!(conflicts.len(), 1);
        assert!(matches!(conflicts[0], Conflict::FileOverlap { .. }));
    }

    #[test]
    fn kind_filter_keeps_only_the_named_kinds() {
        let mut conflicts = sample_conflicts();
        filter_conflicts(
            &parse_cli(&["--kind", "missing_dep", "--kind", "redundant_mod"]),
            &mut conflicts,
        );
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts
            .iter()
            .all(|c| !matches!(c, Conflict::FileOverlap { .. })));
    }

    #[test]
    fn filters_combine_as_and() {
        let mut conflicts = sample_conflicts();
        // beta appears in both the warning and the info conflict, but only the
        // info one survives the kind filter.
        filter_conflicts(
            &parse_cli(&["--mod", "beta", "--kind", "redundant_mod"]),
            &mut conflicts,
        );
        assert_eq!(conflicts.len(), 1);
        assert!(matches!(conflicts[0], Conflict::RedundantMod { .. }));
    }
}
