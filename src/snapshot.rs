//! Frozen reports.
//!
//! The unit tests check that a conflict is found. These check what the user
//! actually reads. A change in wording, ordering or counts shows up as a diff
//! here instead of shipping unnoticed.
//!
//! Regenerate after a deliberate change:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test snapshot
//! ```

#![cfg(test)]

use std::path::{Path, PathBuf};

use crate::analyze::{self, Options};
use crate::report;
use crate::testutil::{info_json, write_folder_mod, write_zip_mod};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_dir().join(name);
    // Keep snapshots newline-stable so they survive a checkout on either OS.
    let actual = actual.replace("\r\n", "\n");

    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::create_dir_all(snapshot_dir()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e}\nRun UPDATE_SNAPSHOTS=1 cargo test snapshot to create it.",
                path.display()
            )
        })
        .replace("\r\n", "\n");

    assert_eq!(
        actual,
        expected,
        "\n{} is out of date. If the change is intended, run:\n  \
         UPDATE_SNAPSHOTS=1 cargo test snapshot\n",
        path.display()
    );
}

/// A Factorio folder with one of every conflict the game can produce.
fn factorio_folder(dir: &Path) {
    let mods = [
        ("base", "1.1.0", vec![]),
        ("alpha", "1.0.0", vec!["base >= 2.0.0", "! gamma"]),
        ("gamma", "1.0.0", vec![]),
        ("delta", "1.0.0", vec!["nonexistent-mod >= 1.0.0"]),
    ];
    for (name, version, deps) in mods {
        write_zip_mod(
            dir,
            &format!("{name}_{version}.zip"),
            &[(
                &format!("{name}_{version}/info.json"),
                &info_json(name, version, &deps),
            )],
        );
    }
    std::fs::write(
        dir.join("mod-list.json"),
        r#"{"mods":[{"name":"base","enabled":true},{"name":"gamma","enabled":true},
                   {"name":"alpha","enabled":true},{"name":"delta","enabled":false}]}"#,
    )
    .unwrap();
}

/// A Fabric folder where two mods overwrite the same asset.
fn fabric_folder(dir: &Path) {
    for name in ["alpha", "beta"] {
        write_folder_mod(
            dir,
            name,
            &[
                (
                    "fabric.mod.json",
                    &format!(r#"{{"id":"{name}","version":"1.0.0"}}"#),
                ),
                ("assets/stone.png", "png"),
            ],
        );
    }
}

#[test]
fn factorio_text_report() {
    let dir = tempfile::tempdir().unwrap();
    factorio_folder(dir.path());

    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_snapshot("factorio.txt", &report::text(&analysis.report()));
}

#[test]
fn factorio_json_report() {
    let dir = tempfile::tempdir().unwrap();
    factorio_folder(dir.path());

    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_snapshot(
        "factorio.json",
        &report::json(&analysis.report()).unwrap(),
    );
}

#[test]
fn fabric_overlap_text_report() {
    let dir = tempfile::tempdir().unwrap();
    fabric_folder(dir.path());

    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_snapshot("fabric.txt", &report::text(&analysis.report()));
}

#[test]
fn a_clean_folder_text_report() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["base", "alpha"] {
        write_zip_mod(
            dir.path(),
            &format!("{name}_1.0.0.zip"),
            &[(
                &format!("{name}_1.0.0/info.json"),
                &info_json(name, "1.0.0", &[]),
            )],
        );
    }

    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_snapshot("clean.txt", &report::text(&analysis.report()));
}

/// The wrong profile on the right mods: everything parses, nothing is
/// understood. The report has to say so rather than claim the folder is clean.
#[test]
fn wrong_profile_warns_about_coverage() {
    let dir = tempfile::tempdir().unwrap();
    factorio_folder(dir.path());

    let analysis = analyze::run(
        dir.path(),
        &Options {
            game: Some("stardew-valley"),
            ..Default::default()
        },
    )
    .unwrap();
    let text = report::text(&analysis.report());

    assert!(text.contains("read metadata for only 0 of"), "{text}");
    assert!(text.contains("wrong game profile"), "{text}");
}
