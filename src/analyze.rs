//! The whole pipeline in one call: scan, pick a profile, parse, detect.
//!
//! Extracted because there are three callers that must agree — the CLI, the
//! end-to-end tests, and the corpus harness. A test that reimplements the
//! pipeline proves the reimplementation works.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::conflict::{self, DetectOptions};
use crate::loadorder;
use crate::model::Conflict;
use crate::parse;
use crate::profile::{self, Profile};
use crate::records;
use crate::scan;

#[derive(Debug, Default, Clone)]
pub struct Options<'a> {
    /// Force a game instead of detecting one.
    pub game: Option<&'a str>,
    pub load_order: Option<&'a Path>,
    pub profiles_dir: Option<&'a Path>,
    /// Off to skip the record-level pass, which is the slow part.
    pub skip_records: bool,
}

pub struct Analysis {
    pub profile: Profile,
    pub mods_scanned: usize,
    /// Mods whose metadata the profile understood, rather than falling back to
    /// the filename. The health signal: far below `mods_scanned` means the
    /// profile is wrong for this folder.
    pub mods_with_metadata: usize,
    pub mods_disabled: usize,
    pub containers: Vec<String>,
    pub plugins_read: usize,
    pub unreadable_plugins: Vec<String>,
    pub load_order_known: bool,
    pub conflicts: Vec<Conflict>,
}

pub fn run(path: &Path, opts: &Options) -> Result<Analysis> {
    let profiles = profile::load_all(opts.profiles_dir)?;
    let raw = scan::scan_dir(path, &profile::metadata_filenames(&profiles))?;

    let profile = match opts.game {
        Some(name) => profile::by_name(&profiles, name)?,
        None => profile::detect(&profiles, &raw)?,
    }
    .clone();

    let load_order = loadorder::read(&profile, path, opts.load_order)?;
    let mut all_mods: Vec<_> = raw.iter().map(|m| parse::parse_mod(&profile, m)).collect();

    // The record pass runs before mods are filtered, because it lines the raw
    // mods up against the parsed ones one to one.
    let records = match (&profile.records, opts.skip_records) {
        (Some(spec), false) => records::scan(spec, &raw, &all_mods),
        _ => records::RecordScan::default(),
    };
    records.enrich(&mut all_mods);

    let ids: Vec<_> = all_mods.iter().map(|m| m.id.clone()).collect();
    let mods_disabled = crate::report::disabled_count(&load_order, &ids);
    // A mod the player switched off cannot conflict with anything.
    let mods: Vec<_> = all_mods
        .into_iter()
        .filter(|m| !load_order.is_disabled(&m.id))
        .collect();

    let options = DetectOptions {
        check_file_overlap: profile.check_file_overlap,
        load_order: load_order.clone(),
        metadata_names: profile::metadata_filenames(std::slice::from_ref(&profile)),
    };
    let mut conflicts = conflict::detect(&mods, &options);

    let enabled: HashSet<&str> = mods.iter().map(|m| m.id.as_str()).collect();
    conflicts.extend(records.overlaps(&enabled));
    conflicts.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| a.title().cmp(&b.title()))
    });

    Ok(Analysis {
        profile,
        mods_scanned: mods.len(),
        mods_with_metadata: mods.iter().filter(|m| m.metadata_found).count(),
        mods_disabled,
        containers: raw
            .iter()
            .flat_map(|m| m.containers.iter().map(|c| c.to_string()))
            .collect(),
        plugins_read: records.plugin_count(),
        unreadable_plugins: records.unreadable.clone(),
        load_order_known: !load_order.is_empty(),
        conflicts,
    })
}

impl Analysis {
    pub fn report(&self) -> crate::report::Report<'_> {
        crate::report::Report {
            profile: &self.profile,
            mods_scanned: self.mods_scanned,
            mods_disabled: self.mods_disabled,
            load_order_known: self.load_order_known,
            containers: self.containers.clone(),
            plugins_read: self.plugins_read,
            mods_with_metadata: self.mods_with_metadata,
            conflicts: &self.conflicts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_of_an_empty_folder_is_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "nothing here").unwrap();

        // Nothing to detect: an empty folder is an error, not a silent pass.
        assert!(run(dir.path(), &Options::default()).is_err());
    }

    #[test]
    fn a_forced_game_skips_detection() {
        let dir = tempfile::tempdir().unwrap();
        crate::testutil::write_folder_mod(dir.path(), "SomeMod", &[("readme.txt", "hi")]);

        let analysis = run(
            dir.path(),
            &Options {
                game: Some("factorio"),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(analysis.profile.name, "factorio");
        // No info.json anywhere, so the profile understood nothing.
        assert_eq!(analysis.mods_with_metadata, 0);
        assert_eq!(analysis.report().metadata_coverage(), 0.0);
    }

    #[test]
    fn coverage_is_full_when_every_mod_is_understood() {
        let dir = tempfile::tempdir().unwrap();
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

        let analysis = run(dir.path(), &Options::default()).unwrap();

        assert_eq!(analysis.profile.name, "factorio");
        assert_eq!(analysis.mods_scanned, 2);
        assert_eq!(analysis.report().metadata_coverage(), 1.0);
    }

    #[test]
    fn an_unreadable_manifest_shows_up_as_missing_coverage() {
        let dir = tempfile::tempdir().unwrap();
        crate::testutil::write_zip_mod(
            dir.path(),
            "good_1.0.0.zip",
            &[(
                "good_1.0.0/info.json",
                &crate::testutil::info_json("good", "1.0.0", &[]),
            )],
        );
        crate::testutil::write_zip_mod(
            dir.path(),
            "bad_1.0.0.zip",
            &[("bad_1.0.0/info.json", "{ not json")],
        );

        let analysis = run(dir.path(), &Options::default()).unwrap();

        assert_eq!(analysis.mods_scanned, 2);
        assert_eq!(analysis.mods_with_metadata, 1);
        assert_eq!(analysis.report().metadata_coverage(), 0.5);
    }
}
