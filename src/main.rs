mod conflict;
mod container;
mod loadorder;
mod model;
mod parse;
mod profile;
mod records;
mod report;
mod scan;
#[cfg(test)]
mod testutil;
mod tui;
mod value;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use conflict::DetectOptions;
use report::Report;

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

    /// Skip the record-level pass. Parsing every plugin is the slow part of a
    /// large Bethesda load order.
    #[arg(long)]
    no_records: bool,

    /// List the games this build knows about, and exit.
    #[arg(long)]
    list_games: bool,
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
    let profiles = profile::load_all(cli.profiles.as_deref())?;

    if cli.list_games {
        for p in &profiles {
            println!("{:<20} {}", p.name, p.display_name);
        }
        return Ok(true);
    }

    let raw = scan::scan_dir(&cli.path, &profile::metadata_filenames(&profiles))?;
    let profile = match cli.game.as_deref() {
        Some(name) => profile::by_name(&profiles, name)?,
        None => profile::detect(&profiles, &raw)?,
    };

    let load_order = loadorder::read(profile, &cli.path, cli.load_order.as_deref())?;
    let mut all_mods: Vec<_> = raw.iter().map(|m| parse::parse_mod(profile, m)).collect();

    // The record pass runs before mods are filtered, because it needs the raw
    // mods and the parsed ones lined up one to one.
    let records = match (&profile.records, cli.no_records) {
        (Some(spec), false) => records::scan(spec, &raw, &all_mods),
        _ => records::RecordScan::default(),
    };
    records.enrich(&mut all_mods);
    for problem in &records.unreadable {
        eprintln!("warning: cannot read plugin {problem}");
    }

    let ids: Vec<_> = all_mods.iter().map(|m| m.id.clone()).collect();
    let mods_disabled = report::disabled_count(&load_order, &ids);
    // A mod the player switched off cannot conflict with anything.
    let mods: Vec<_> = all_mods
        .into_iter()
        .filter(|m| !load_order.is_disabled(&m.id))
        .collect();

    let options = DetectOptions {
        check_file_overlap: profile.check_file_overlap,
        load_order: load_order.clone(),
    };
    let mut conflicts = conflict::detect(&mods, &options);

    let enabled: std::collections::HashSet<&str> = mods.iter().map(|m| m.id.as_str()).collect();
    conflicts.extend(records.overlaps(&enabled));
    conflicts.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| a.title().cmp(&b.title()))
    });

    let report = Report {
        profile,
        mods_scanned: mods.len(),
        mods_disabled,
        load_order_known: !load_order.is_empty(),
        containers: raw
            .iter()
            .flat_map(|m| m.containers.iter().map(|c| c.to_string()))
            .collect(),
        plugins_read: records.plugin_count(),
        conflicts: &conflicts,
    };

    if cli.tui {
        tui::run(conflicts.clone())?;
    } else if cli.json {
        report::print_json(&report)?;
    } else {
        report::print_text(&report);
    }
    Ok(report.is_clean())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Conflict;
    use crate::testutil::{info_json, metadata_names, write_zip_mod};
    use std::path::Path;

    /// Scan, detect the game, parse, and report — over real zips on disk.
    fn analyze(dir: &Path, load_order: Option<&Path>) -> (String, Vec<Conflict>) {
        let profiles = profile::load_all(None).unwrap();
        let raw = scan::scan_dir(dir, &metadata_names()).unwrap();
        let profile = profile::detect(&profiles, &raw).unwrap();
        let order = loadorder::read(profile, dir, load_order).unwrap();

        let mods: Vec<_> = raw
            .iter()
            .map(|m| parse::parse_mod(profile, m))
            .filter(|m| !order.is_disabled(&m.id))
            .collect();

        let options = DetectOptions {
            check_file_overlap: profile.check_file_overlap,
            load_order: order,
        };
        (profile.name.clone(), conflict::detect(&mods, &options))
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
        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap();
        let profile = profile::detect(&profiles, &raw).unwrap();

        assert_eq!(profile.name, "minecraft-fabric");
        assert!(loadorder::read(profile, dir.path(), Some(&order)).is_err());

        // Without a load order the overlap is still reported, winner unknown.
        let (_, conflicts) = analyze(dir.path(), None);
        assert_eq!(conflicts.len(), 1);
        assert!(matches!(
            &conflicts[0],
            Conflict::FileOverlap { winner: None, .. }
        ));
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
            &[("textures/armor/iron.dds", "different bytes"), ("Loose.esp", "TES4")],
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

        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap();

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

        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap();

        assert_eq!(raw.len(), 1);
        assert!(raw[0].containers.is_empty());
        assert!(raw[0].files.iter().any(|f| f == "Broken.esp"));
    }

    #[test]
    fn detection_fails_loudly_on_an_unrecognized_folder() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(dir.path(), "mystery.zip", &[("readme.txt", "hello")]);

        let profiles = profile::load_all(None).unwrap();
        let raw = scan::scan_dir(dir.path(), &metadata_names()).unwrap();

        assert!(profile::detect(&profiles, &raw).is_err());
    }
}
