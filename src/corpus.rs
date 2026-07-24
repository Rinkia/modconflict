//! Validation against real mod folders.
//!
//! Every other test in this crate uses fixtures built from format
//! documentation. That proves the profiles match what the docs say — not that
//! the docs match what modders actually publish. Only real mods answer that,
//! and real mods cannot live in this repository: they are other people's work
//! under other people's licences, and they are large.
//!
//! So the corpus lives on the machine of whoever runs it, and this harness is
//! opt-in:
//!
//! ```text
//! MODCONFLICT_CORPUS=/path/to/corpus cargo test --ignored corpus
//! ```
//!
//! The directory holds a `corpus.toml` describing what is in it and what the
//! tool must manage on it. See the README for the format.
//!
//! The assertions are about the health of the *tool*, never the cleanliness of
//! the mods: a real folder is expected to have conflicts, and a harness that
//! failed on them would be useless.

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

use crate::analyze::{self, Options};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    #[serde(default, rename = "entry")]
    entries: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// Folder holding the mods, relative to the corpus directory or absolute.
    path: String,
    /// The profile detection must land on. Omit to accept whatever it picks.
    #[serde(default)]
    game: Option<String>,
    /// Fail if the folder yields fewer mods than this — catches a scan that
    /// silently found nothing.
    #[serde(default)]
    min_mods: usize,
    /// Fail if the profile understood a smaller share of the mods. This is the
    /// real signal: a profile that is subtly wrong still parses, still reports,
    /// and quietly falls back to filenames for most of the folder.
    #[serde(default = "default_coverage")]
    min_metadata_coverage: f64,
    /// Fail if a scan takes longer than this.
    #[serde(default = "default_seconds")]
    max_seconds: f64,
    /// Plugins allowed to be unreadable before it counts as a failure.
    #[serde(default)]
    max_unreadable_plugins: usize,
    #[serde(default)]
    skip_records: bool,
}

fn default_coverage() -> f64 {
    0.95
}

fn default_seconds() -> f64 {
    60.0
}

fn corpus_dir() -> Option<PathBuf> {
    let raw = std::env::var("MODCONFLICT_CORPUS").ok()?;
    (!raw.trim().is_empty()).then(|| PathBuf::from(raw))
}

fn check(dir: &Path, entry: &Entry) -> Result<String, String> {
    let path = {
        let candidate = PathBuf::from(&entry.path);
        if candidate.is_absolute() {
            candidate
        } else {
            dir.join(&entry.path)
        }
    };
    if !path.is_dir() {
        return Err(format!("{}: not a directory", path.display()));
    }

    let started = Instant::now();
    let analysis = analyze::run(
        &path,
        &Options {
            game: entry.game.as_deref(),
            skip_records: entry.skip_records,
            ..Default::default()
        },
    )
    .map_err(|e| format!("{}: {e:#}", path.display()))?;
    let elapsed = started.elapsed().as_secs_f64();

    let mut problems = Vec::new();

    if let Some(expected) = &entry.game {
        if &analysis.profile.name != expected {
            problems.push(format!(
                "detected \"{}\", expected \"{expected}\"",
                analysis.profile.name
            ));
        }
    }
    if analysis.mods_scanned < entry.min_mods {
        problems.push(format!(
            "found {} mods, expected at least {}",
            analysis.mods_scanned, entry.min_mods
        ));
    }
    let coverage = analysis.report().metadata_coverage();
    if coverage < entry.min_metadata_coverage {
        problems.push(format!(
            "understood the metadata of only {:.0}% of mods ({} of {}), expected {:.0}%",
            coverage * 100.0,
            analysis.mods_with_metadata,
            analysis.mods_scanned,
            entry.min_metadata_coverage * 100.0
        ));
    }
    if analysis.unreadable_plugins.len() > entry.max_unreadable_plugins {
        problems.push(format!(
            "{} unreadable plugins, allowed {}: {}",
            analysis.unreadable_plugins.len(),
            entry.max_unreadable_plugins,
            analysis.unreadable_plugins.join("; ")
        ));
    }
    if elapsed > entry.max_seconds {
        problems.push(format!(
            "took {elapsed:.1}s, budget {:.1}s",
            entry.max_seconds
        ));
    }

    let summary = format!(
        "{}: {} [{} mods, {:.0}% understood, {} conflicts, {elapsed:.1}s]",
        entry.path,
        analysis.profile.name,
        analysis.mods_scanned,
        coverage * 100.0,
        analysis.conflicts.len(),
    );

    if problems.is_empty() {
        Ok(summary)
    } else {
        Err(format!("{summary}\n    - {}", problems.join("\n    - ")))
    }
}

#[test]
#[ignore = "needs a real mod corpus; set MODCONFLICT_CORPUS"]
fn the_tool_survives_real_mod_folders() {
    let Some(dir) = corpus_dir() else {
        panic!(
            "MODCONFLICT_CORPUS is not set. Point it at a directory holding a \
             corpus.toml — see the README."
        );
    };

    let manifest = dir.join("corpus.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
    let corpus: Corpus =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));

    assert!(
        !corpus.entries.is_empty(),
        "{}: no [[entry]] sections",
        manifest.display()
    );

    let mut failures = Vec::new();
    for entry in &corpus.entries {
        // Every entry runs even after one fails: the point is a full picture of
        // where the tool stands, not the first thing that broke.
        match check(&dir, entry) {
            Ok(summary) => println!("ok    {summary}"),
            Err(problem) => {
                println!("FAIL  {problem}");
                failures.push(problem);
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} corpus entries failed",
        failures.len(),
        corpus.entries.len()
    );
}

#[test]
fn the_corpus_manifest_format_parses() {
    // Guards the harness itself, since the real test never runs in CI.
    let corpus: Corpus = toml::from_str(
        r#"
[[entry]]
path = "factorio"
game = "factorio"
min_mods = 20

[[entry]]
path = "/absolute/skyrim/mods"
game = "creation-engine"
min_metadata_coverage = 0.0
max_seconds = 120.0
max_unreadable_plugins = 2
skip_records = true
"#,
    )
    .unwrap();

    assert_eq!(corpus.entries.len(), 2);
    assert_eq!(corpus.entries[0].game.as_deref(), Some("factorio"));
    // Defaults apply where the entry is silent.
    assert_eq!(corpus.entries[0].min_metadata_coverage, 0.95);
    assert_eq!(corpus.entries[0].max_seconds, 60.0);
    assert!(!corpus.entries[0].skip_records);
    assert!(corpus.entries[1].skip_records);
}

#[test]
fn a_missing_corpus_directory_is_reported_not_silently_passed() {
    let entry = Entry {
        path: "nowhere".into(),
        game: None,
        min_mods: 0,
        min_metadata_coverage: 0.95,
        max_seconds: 60.0,
        max_unreadable_plugins: 0,
        skip_records: false,
    };

    let err = check(Path::new("/definitely/not/here"), &entry).unwrap_err();

    assert!(err.contains("not a directory"), "{err}");
}

#[test]
fn a_folder_the_profile_does_not_understand_fails_the_check() {
    let dir = tempfile::tempdir().unwrap();
    // Factorio mods, but scanned as if they were Stardew: everything parses,
    // nothing is understood. Exactly the silent failure the corpus exists to
    // catch.
    for name in ["alpha", "beta"] {
        crate::testutil::write_zip_mod(
            dir.path(),
            &format!("{name}_1.0.0.zip"),
            &[(
                &format!("{name}_1.0.0/info.json"),
                &crate::testutil::info_json(name, "1.0.0", &[]),
            )],
        );
    }

    let entry = Entry {
        path: dir.path().to_string_lossy().into_owned(),
        game: Some("stardew-valley".into()),
        min_mods: 2,
        min_metadata_coverage: 0.95,
        max_seconds: 60.0,
        max_unreadable_plugins: 0,
        skip_records: false,
    };

    let err = check(dir.path(), &entry).unwrap_err();

    assert!(err.contains("understood the metadata of only 0%"), "{err}");
}
