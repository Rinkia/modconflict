mod analyze;
mod baseline;
mod bg3;
mod conflict;
mod container;
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod fixtures;
mod hash;
#[cfg(test)]
mod hostile;
mod ignore;
mod limits;
mod loadorder;
mod manager;
mod model;
mod parse;
mod profile;
mod records;
mod report;
mod scan;
#[cfg(test)]
mod snapshot;
#[cfg(test)]
mod testutil;
mod tui;
mod value;
mod versionreq;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

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
    use crate::model::Conflict;
    use crate::testutil::{info_json, metadata_names, write_zip_mod};
    use std::path::Path;

    /// The same pipeline the CLI runs, over real archives on disk.
    fn analyze(dir: &Path, load_order: Option<&Path>) -> (String, Vec<Conflict>) {
        let analysis = analyze::run(
            dir,
            &analyze::Options {
                load_order,
                ..Default::default()
            },
        )
        .unwrap();
        (analysis.profile.name.clone(), analysis.conflicts)
    }

    fn factorio_mod(dir: &Path, name: &str, version: &str, deps: &[&str]) {
        write_zip_mod(
            dir,
            &format!("{name}_{version}.zip"),
            &[(
                &format!("{name}_{version}/info.json"),
                &info_json(name, version, deps),
            )],
        );
    }

    fn sample_conflicts() -> Vec<Conflict> {
        use crate::model::{Dep, DepKind};
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

    #[test]
    fn finds_the_planted_conflicts_in_a_factorio_folder() {
        let dir = tempfile::tempdir().unwrap();
        factorio_mod(dir.path(), "base", "1.1.0", &[]);
        factorio_mod(dir.path(), "alpha", "1.0.0", &["base >= 2.0.0", "! gamma"]);
        factorio_mod(dir.path(), "gamma", "1.0.0", &[]);
        factorio_mod(dir.path(), "delta", "1.0.0", &["nonexistent-mod >= 1.0.0"]);

        let (game, conflicts) = analyze(dir.path(), None);

        assert_eq!(game, "factorio");
        assert_eq!(conflicts.len(), 3);
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::VersionMismatch { .. })));
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::Incompatible { .. })));
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::MissingDep { .. })));
    }

    #[test]
    fn a_clean_folder_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        factorio_mod(dir.path(), "base", "1.1.0", &[]);
        factorio_mod(dir.path(), "alpha", "1.0.0", &["base >= 1.0.0"]);

        assert!(analyze(dir.path(), None).1.is_empty());
    }

    #[test]
    fn a_mod_disabled_in_the_load_order_cannot_conflict() {
        let dir = tempfile::tempdir().unwrap();
        factorio_mod(dir.path(), "base", "1.1.0", &[]);
        factorio_mod(dir.path(), "alpha", "1.0.0", &["base >= 2.0.0"]);
        std::fs::write(
            dir.path().join("mod-list.json"),
            r#"{"mods":[{"name":"base","enabled":true},{"name":"alpha","enabled":false}]}"#,
        )
        .unwrap();

        // alpha is the only source of conflict, and it is switched off.
        assert!(analyze(dir.path(), None).1.is_empty());
    }

    #[test]
    fn the_load_order_decides_who_wins_an_overlap() {
        let dir = tempfile::tempdir().unwrap();
        // Fabric checks file overlap, unlike Factorio.
        for (name, version) in [("alpha", "1.0.0"), ("beta", "1.0.0")] {
            write_zip_mod(
                dir.path(),
                &format!("{name}.jar"),
                &[
                    (
                        "fabric.mod.json",
                        &format!(r#"{{"id":"{name}","version":"{version}"}}"#),
                    ),
                    ("assets/stone.png", "png"),
                ],
            );
        }
        let order = dir.path().join("order.txt");
        std::fs::write(&order, "beta\nalpha\n").unwrap();

        // Fabric has no load_order section, so an explicit file is refused
        // rather than silently ignored.
        let profiles = profile::load_all(None).unwrap();
        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap().mods;
        let profile = profile::detect(&profiles, &raw).unwrap();

        assert_eq!(profile.name, "minecraft-fabric");
        assert!(loadorder::read(profile, dir.path(), Some(&order)).is_err());

        // Without a load order the overlap is still reported, winner unknown.
        let (_, conflicts) = analyze(dir.path(), None);
        assert!(conflicts
            .iter()
            .any(|c| matches!(c, Conflict::FileOverlap { winner: None, .. })));
    }

    /// The point of the container layer: a texture packed inside a `.bsa` and a
    /// loose copy of the same texture in another mod are the same conflict the
    /// game sees, and a filename-only scan cannot see it at all.
    #[test]
    fn a_file_inside_a_bethesda_archive_collides_with_a_loose_copy() {
        use crate::testutil::{write_bsa, write_folder_mod};

        let dir = tempfile::tempdir().unwrap();

        // Mod one: an archive containing the texture, plus a plugin.
        let packed = dir.path().join("PackedMod");
        std::fs::create_dir(&packed).unwrap();
        std::fs::write(packed.join("Packed.esp"), b"TES4").unwrap();
        write_bsa(
            &packed.join("Packed.bsa"),
            &["textures/armor/iron.dds", "meshes/armor/iron.nif"],
        );

        // Mod two: the same texture, loose.
        write_folder_mod(
            dir.path(),
            "LooseMod",
            &[
                ("textures/armor/iron.dds", "different bytes"),
                ("Loose.esp", "TES4"),
            ],
        );

        let (game, conflicts) = analyze(dir.path(), None);

        assert_eq!(game, "creation-engine");
        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::FileOverlap { path, mods, .. } => {
                assert_eq!(path, "textures/armor/iron.dds");
                assert_eq!(mods, &["LooseMod".to_string(), "PackedMod".to_string()]);
            }
            other => panic!("expected a file overlap, got {other:?}"),
        }
    }

    #[test]
    fn the_scan_records_which_binary_archives_it_expanded() {
        use crate::testutil::write_bsa;

        let dir = tempfile::tempdir().unwrap();
        let mod_dir = dir.path().join("SomeMod");
        std::fs::create_dir(&mod_dir).unwrap();
        std::fs::write(mod_dir.join("Some.esp"), b"TES4").unwrap();
        write_bsa(&mod_dir.join("Some.bsa"), &["textures/a.dds"]);

        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap().mods;

        assert_eq!(raw[0].containers, vec!["Bethesda archive"]);
        assert!(raw[0].files.iter().any(|f| f == "textures/a.dds"));
        // The archive itself is still listed alongside its contents.
        assert!(raw[0].files.iter().any(|f| f == "Some.bsa"));
    }

    #[test]
    fn an_unreadable_archive_does_not_sink_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let mod_dir = dir.path().join("BrokenMod");
        std::fs::create_dir(&mod_dir).unwrap();
        std::fs::write(mod_dir.join("Broken.bsa"), b"BSA\0truncated").unwrap();
        std::fs::write(mod_dir.join("Broken.esp"), b"TES4").unwrap();

        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap().mods;

        assert_eq!(raw.len(), 1);
        assert!(raw[0].containers.is_empty());
        assert!(raw[0].files.iter().any(|f| f == "Broken.esp"));
    }

    /// A BG3 mod is a bare `.pak` in the folder, with its metadata locked
    /// inside it — the whole path from "file the scanner would have ignored"
    /// to "dependency reported by UUID".
    #[test]
    fn a_folder_of_bg3_paks_is_detected_and_its_uuids_resolved() {
        use crate::testutil::{bg3_meta_lsx, write_bg3_pak};

        const BASE: &str = "33333333-3333-3333-3333-333333333333";
        const PATCH: &str = "22222222-2222-2222-2222-222222222222";
        const ABSENT: &str = "99999999-9999-9999-9999-999999999999";

        let dir = tempfile::tempdir().unwrap();
        write_bg3_pak(
            dir.path(),
            "BaseMod",
            &bg3_meta_lsx(BASE, "BaseMod", 36028797018963968, &[]),
        );
        write_bg3_pak(
            dir.path(),
            "PatchMod",
            &bg3_meta_lsx(PATCH, "PatchMod", 36028797018963968, &[(BASE, "BaseMod")]),
        );
        write_bg3_pak(
            dir.path(),
            "BrokenPatch",
            &bg3_meta_lsx(
                "44444444-4444-4444-4444-444444444444",
                "BrokenPatch",
                36028797018963968,
                &[(ABSENT, "GoneMod")],
            ),
        );

        let (game, conflicts) = analyze(dir.path(), None);

        assert_eq!(game, "baldurs-gate-3");
        // The satisfied dependency is silent; only the absent one is reported.
        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::MissingDep { mod_id, dep } => {
                assert_eq!(mod_id, "44444444-4444-4444-4444-444444444444");
                assert_eq!(dep.name, ABSENT);
            }
            other => panic!("expected a missing dependency, got {other:?}"),
        }
    }

    /// The payoff of reading a mod manager: the Creation Engine profile has no
    /// load order of its own on purpose, so without MO2 the report can say two
    /// mods clash but never which one survives. With it, both winners are named.
    #[cfg(feature = "records")]
    #[test]
    fn mod_organizer_decides_both_kinds_of_winner() {
        use crate::testutil::{write_folder_mod, write_plugin};

        let root = tempfile::tempdir().unwrap();
        let mods = root.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();

        // Two mods editing the same record and shipping the same texture.
        for (name, plugin) in [("LoserMod", "Loser.esp"), ("WinnerMod", "Winner.esp")] {
            write_folder_mod(&mods, name, &[("textures/iron.dds", name)]);
            write_plugin(
                &mods.join(name).join(plugin),
                &["Skyrim.esm"],
                &[0x0000_0ABC],
            );
        }

        // MO2 writes highest priority first, so WinnerMod on top wins.
        let profile = root.path().join("profiles").join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("modlist.txt"),
            "+WinnerMod
+LoserMod
",
        )
        .unwrap();
        std::fs::write(
            profile.join("plugins.txt"),
            "*Skyrim.esm
*Loser.esp
*Winner.esp
",
        )
        .unwrap();
        std::fs::write(
            root.path().join("ModOrganizer.ini"),
            "[General]
selected_profile=Default
",
        )
        .unwrap();

        let analysis = analyze::run(&mods, &analyze::Options::default()).unwrap();

        let manager = analysis.manager.as_ref().expect("MO2 should be detected");
        assert_eq!(manager.profile, "Default");

        let overlap = analysis
            .conflicts
            .iter()
            .find(|c| matches!(c, Conflict::FileOverlap { .. }))
            .expect("the shared texture must be reported");
        match overlap {
            Conflict::FileOverlap { path, winner, .. } => {
                assert_eq!(path, "textures/iron.dds");
                assert_eq!(winner.as_deref(), Some("WinnerMod"));
            }
            other => panic!("expected a file overlap, got {other:?}"),
        }

        let record = analysis
            .conflicts
            .iter()
            .find(|c| matches!(c, Conflict::RecordOverlap { .. }))
            .expect("the shared record must be reported");
        match record {
            Conflict::RecordOverlap {
                winner, records, ..
            } => {
                assert_eq!(*records, 1);
                assert_eq!(winner.as_deref(), Some("Winner.esp"));
            }
            other => panic!("expected a record overlap, got {other:?}"),
        }
    }

    /// The same folder without the manager: the clashes are still found, but
    /// nothing claims to know who wins.
    #[cfg(feature = "records")]
    #[test]
    fn without_a_manager_no_winner_is_invented() {
        use crate::testutil::{write_folder_mod, write_plugin};

        let dir = tempfile::tempdir().unwrap();
        for (name, plugin) in [("ModA", "A.esp"), ("ModB", "B.esp")] {
            write_folder_mod(dir.path(), name, &[("textures/iron.dds", name)]);
            write_plugin(
                &dir.path().join(name).join(plugin),
                &["Skyrim.esm"],
                &[0x0000_0ABC],
            );
        }

        let analysis = analyze::run(dir.path(), &analyze::Options::default()).unwrap();

        assert!(analysis.manager.is_none());
        assert!(analysis
            .conflicts
            .iter()
            .any(|c| matches!(c, Conflict::FileOverlap { winner: None, .. })));
        assert!(analysis
            .conflicts
            .iter()
            .any(|c| matches!(c, Conflict::RecordOverlap { winner: None, .. })));
    }

    #[test]
    fn detection_fails_loudly_on_an_unrecognized_folder() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(dir.path(), "mystery.zip", &[("readme.txt", "hello")]);

        let profiles = profile::load_all(None).unwrap();
        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap().mods;

        assert!(profile::detect(&profiles, &raw).is_err());
    }
}
