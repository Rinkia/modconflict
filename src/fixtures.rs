//! Every profile must prove itself.
//!
//! A profile is a claim about a game's metadata format, and a wrong claim fails
//! silently: the mod parses, the id is wrong, and the report is confidently
//! useless. So each profile ships a sample metadata file and the exact result
//! it must produce, and the test below fails when a profile has no fixture at
//! all — a new game cannot be added without evidence.
//!
//! What a fixture proves is that the profile matches the format *as documented*.
//! It does not prove the documentation matches the mods people actually publish;
//! only a corpus of real mods does that.

#![cfg(test)]

use std::path::PathBuf;

use serde::Deserialize;

use crate::model::{DepKind, SymbolKind};
use crate::parse::parse_mod;
use crate::profile::{load_all, Profile};
use crate::scan::RawMod;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    /// Where the format claim comes from, so a wrong fixture can be traced.
    #[allow(dead_code)]
    source_of_truth: String,
    /// Filename of the mod archive or folder, for the id fallback.
    #[serde(default = "default_source")]
    mod_source: String,
    /// Path the metadata file has *inside the mod*, nesting included, since
    /// that is what the scanner sees. The fixture file itself lives flat in
    /// `input/` under the basename.
    metadata_path: String,
    id: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    requires: Vec<ExpectedDep>,
}

fn default_source() -> String {
    "FixtureMod.zip".to_string()
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExpectedDep {
    name: String,
    #[serde(default)]
    req: Option<String>,
    kind: String,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("profiles/fixtures")
}

fn kind_name(kind: DepKind) -> &'static str {
    match kind {
        DepKind::Required => "required",
        DepKind::Optional => "optional",
        DepKind::Incompatible => "incompatible",
    }
}

/// Run one profile against its fixture.
fn check(profile: &Profile) {
    let dir = fixtures_dir().join(&profile.name);
    let expected_path = dir.join("expected.json");

    let expected: Expected = serde_json::from_str(
        &std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("{}: {e}", expected_path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", expected_path.display()));

    let basename = expected
        .metadata_path
        .rsplit('/')
        .next()
        .unwrap_or(&expected.metadata_path);
    let metadata = std::fs::read_to_string(dir.join("input").join(basename))
        .unwrap_or_else(|e| panic!("{}/input/{basename}: {e}", dir.display()));

    let raw = RawMod::from_files(
        PathBuf::from(&expected.mod_source),
        &[(expected.metadata_path.as_str(), metadata.as_str())],
        &crate::profile::metadata_filenames(std::slice::from_ref(profile)),
    );

    let entry = parse_mod(profile, &raw);
    let name = &profile.name;

    assert_eq!(entry.id, expected.id, "[{name}] mod id");
    assert_eq!(entry.version, expected.version, "[{name}] version");

    let provides: Vec<&str> = entry
        .provides
        .iter()
        .filter(|s| s.kind == SymbolKind::ModId)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(provides, expected.provides, "[{name}] provides");

    let mut got: Vec<ExpectedDep> = entry
        .requires
        .iter()
        .map(|d| ExpectedDep {
            name: d.name.clone(),
            req: d.req.clone(),
            kind: kind_name(d.kind).to_string(),
        })
        .collect();
    let mut want = expected.requires;
    got.sort_by(|a, b| a.name.cmp(&b.name));
    want.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(got, want, "[{name}] dependencies");
}

#[test]
fn every_profile_with_a_metadata_file_has_a_fixture_and_matches_it() {
    let profiles = load_all(None).unwrap();
    let mut checked = 0;

    for profile in &profiles {
        // A profile with no metadata file has nothing to parse — the Creation
        // Engine one is covered by the record tests instead.
        if profile.metadata_file.is_none() {
            continue;
        }
        let dir = fixtures_dir().join(&profile.name);
        assert!(
            dir.is_dir(),
            "profile \"{}\" has no fixture. Add profiles/fixtures/{}/ with an \
             input/ metadata file and expected.json before shipping it.",
            profile.name,
            profile.name
        );
        check(profile);
        checked += 1;
    }

    assert!(checked > 0, "no profiles were checked — is the fixture path right?");
}

#[test]
fn a_fixture_directory_without_a_profile_is_a_leftover() {
    let profiles = load_all(None).unwrap();
    let dir = fixtures_dir();

    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // Editor and OS scratch directories are not anyone's fixture.
        if name.starts_with('.') {
            continue;
        }
        assert!(
            profiles.iter().any(|p| p.name == name),
            "fixture \"{name}\" has no matching profile — delete it or add the profile"
        );
    }
}
