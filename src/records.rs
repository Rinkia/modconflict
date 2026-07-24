//! The record level, for Creation Engine games.
//!
//! The container layer answers "which files does this mod ship". That misses
//! the conflict Bethesda players actually hit: two plugins editing the *same
//! record* — the same NPC, the same weapon, the same cell. Nothing about the
//! filenames reveals it; it lives in the binary plugin.
//!
//! `esplugin` — the library behind LOOT — does the parsing. What this module
//! adds is the translation into the shared model, so record conflicts are
//! reported next to every other kind instead of in a separate world.

use std::collections::{HashMap, HashSet};
#[cfg(feature = "records")]
use std::path::Path;

#[cfg(feature = "records")]
use esplugin::{GameId, ParseOptions, Plugin};

use crate::manager::ManagerData;
use crate::model::{Conflict, Dep, DepKind, ModEntry, ModId, Symbol, SymbolKind};
use crate::profile::RecordSpec;
use crate::scan::RawMod;

/// What the record pass learned, ready to be folded into the shared model.
#[derive(Debug, Default)]
pub struct RecordScan {
    /// Plugin filenames each mod installs.
    provides: Vec<(ModId, String)>,
    /// Masters each mod's plugins require, with the game's own masters removed.
    requires: Vec<(ModId, String)>,
    /// Pairs of plugins from different mods that edit the same records.
    overlaps: Vec<Conflict>,
    /// Plugins that could not be parsed, for a note in the report.
    pub unreadable: Vec<String>,
}

impl RecordScan {
    /// Fold plugin filenames and masters into the mods' symbols and
    /// dependencies, so the existing detector finds duplicate plugins and
    /// missing masters with no new code.
    pub fn enrich(&self, mods: &mut [ModEntry]) {
        let index: HashMap<String, usize> = mods
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.clone(), i))
            .collect();

        for (mod_id, plugin) in &self.provides {
            if let Some(&i) = index.get(mod_id) {
                mods[i].provides.push(Symbol {
                    kind: SymbolKind::PluginFile,
                    name: plugin.clone(),
                });
            }
        }
        for (mod_id, master) in &self.requires {
            if let Some(&i) = index.get(mod_id) {
                mods[i].requires.push(Dep {
                    name: master.clone(),
                    req: None,
                    kind: DepKind::Required,
                });
            }
        }
    }

    /// Record overlaps, minus any involving a mod that is switched off.
    pub fn overlaps(&self, enabled: &HashSet<&str>) -> Vec<Conflict> {
        self.overlaps
            .iter()
            .filter(|c| match c {
                Conflict::RecordOverlap { mods, .. } => {
                    mods.iter().all(|m| enabled.contains(m.as_str()))
                }
                _ => true,
            })
            .cloned()
            .collect()
    }

    pub fn plugin_count(&self) -> usize {
        self.provides.len()
    }
}

/// Without the `records` feature there is no plugin parser, so the pass finds
/// nothing and everything downstream carries on unchanged. See
/// THIRD-PARTY-NOTICES.md for why the feature exists.
#[cfg(not(feature = "records"))]
pub fn scan(
    _spec: &RecordSpec,
    _raw: &[RawMod],
    _mods: &[ModEntry],
    _manager: Option<&ManagerData>,
) -> RecordScan {
    RecordScan::default()
}

#[cfg(feature = "records")]
struct Loaded {
    mod_id: ModId,
    filename: String,
    plugin: Plugin,
}

/// Parse every plugin in every mod, then compare them pairwise.
///
/// Plugins that fail to parse are collected and skipped: a single malformed
/// `.esp` must not cost the user the rest of the report.
#[cfg(feature = "records")]
pub fn scan(
    spec: &RecordSpec,
    raw: &[RawMod],
    mods: &[ModEntry],
    manager: Option<&ManagerData>,
) -> RecordScan {
    let game = spec.game_id();
    let base: HashSet<String> = spec
        .base_ids
        .iter()
        .map(|m| m.to_ascii_lowercase())
        .collect();

    let mut result = RecordScan::default();
    let mut loaded: Vec<Loaded> = Vec::new();

    for (raw_mod, entry) in raw.iter().zip(mods) {
        // Plugins are parsed from disk, so a mod that is still a .zip has no
        // readable plugin paths. Creation Engine mods are installed as folders.
        if !raw_mod.source.is_dir() {
            continue;
        }

        for rel in plugin_files(spec, &raw_mod.files) {
            let path = raw_mod.source.join(rel);
            let filename = file_name(&path);

            result.provides.push((entry.id.clone(), filename.clone()));

            match load(game, &path) {
                Ok(plugin) => loaded.push(Loaded {
                    mod_id: entry.id.clone(),
                    filename,
                    plugin,
                }),
                Err(e) => result.unreadable.push(format!("{}: {e}", path.display())),
            }
        }
    }

    for entry in &loaded {
        for master in entry.plugin.masters().unwrap_or_default() {
            if base.contains(&master.to_ascii_lowercase()) {
                continue;
            }
            result.requires.push((entry.mod_id.clone(), master));
        }
    }

    resolve(&mut loaded);
    result.overlaps = compare(&loaded, manager);
    result
}

/// Behind a panic boundary for the same reason the container readers are: a
/// plugin is hostile binary input, and one malformed file must not cost the
/// other two hundred.
#[cfg(feature = "records")]
fn load(game: GameId, path: &Path) -> anyhow::Result<Plugin> {
    let owned = path.to_path_buf();
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut plugin = Plugin::new(game, &owned);
        // ponytail: whole-plugin parse holds every record id in memory — a few
        // hundred MB for a 200-plugin load order. Stream it only if that bites.
        plugin.parse_file(ParseOptions::whole_plugin())?;
        Ok::<_, esplugin::Error>(plugin)
    }));

    match parsed {
        Ok(Ok(plugin)) => Ok(plugin),
        Ok(Err(e)) => Err(anyhow::anyhow!("{e}")),
        Err(_) => anyhow::bail!("the plugin parser panicked on this file"),
    }
}

/// FormIDs are stored relative to each plugin's own master list, so they mean
/// nothing across plugins until they are resolved against it. Skipping this
/// step would compare two different numbering schemes and call it an overlap.
#[cfg(feature = "records")]
fn resolve(loaded: &mut [Loaded]) {
    let refs: Vec<&Plugin> = loaded.iter().map(|l| &l.plugin).collect();
    let Ok(metadata) = esplugin::plugins_metadata(&refs) else {
        return;
    };
    for entry in loaded.iter_mut() {
        let _ = entry.plugin.resolve_record_ids(&metadata);
    }
}

#[cfg(feature = "records")]
fn compare(loaded: &[Loaded], manager: Option<&ManagerData>) -> Vec<Conflict> {
    let mut conflicts = Vec::new();

    // ponytail: O(n^2) pairwise. Fine up to a few hundred plugins, which is a
    // large load order; bucket by master if it ever is not.
    for (i, a) in loaded.iter().enumerate() {
        for b in loaded.iter().skip(i + 1) {
            // Two plugins from the same mod are shipped together on purpose.
            if a.mod_id == b.mod_id {
                continue;
            }
            if !a.plugin.overlaps_with(&b.plugin).unwrap_or(false) {
                continue;
            }

            let records = a.plugin.overlap_size(&[&b.plugin]).unwrap_or(0);
            let mut mods = vec![a.mod_id.clone(), b.mod_id.clone()];
            mods.sort();

            let plugins = vec![a.filename.clone(), b.filename.clone()];
            // Only a mod manager knows the plugin order, so without one the
            // report says two plugins clash but not which survives.
            let winner = manager
                .and_then(|m| m.plugin_winner(&plugins))
                .cloned();

            conflicts.push(Conflict::RecordOverlap {
                plugins,
                mods,
                records,
                winner,
            });
        }
    }
    conflicts
}

#[cfg(feature = "records")]
fn plugin_files<'a>(spec: &'a RecordSpec, files: &'a [String]) -> impl Iterator<Item = &'a str> {
    files.iter().filter_map(move |f| {
        let (_, ext) = f.rsplit_once('.')?;
        spec.extensions
            .iter()
            .any(|want| want.eq_ignore_ascii_case(ext))
            .then_some(f.as_str())
    })
}

#[cfg(feature = "records")]
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(all(test, feature = "records"))]
mod tests {
    use super::*;
    use crate::profile::{by_name, load_all, Profile};
    use crate::testutil::{metadata_names, write_plugin};
    use crate::{parse, scan as scanner};

    fn creation_engine() -> Profile {
        by_name(&load_all(None).unwrap(), "creation-engine")
            .unwrap()
            .clone()
    }

    /// Build a mod folder holding one plugin.
    fn mod_with_plugin(root: &Path, mod_name: &str, plugin: &str, masters: &[&str], forms: &[u32]) {
        let dir = root.join(mod_name);
        std::fs::create_dir_all(&dir).unwrap();
        write_plugin(&dir.join(plugin), masters, forms);
    }

    fn run(dir: &Path) -> (RecordScan, Vec<ModEntry>) {
        let profile = creation_engine();
        let raw = scanner::scan_dir(dir, &metadata_names()).unwrap().mods;
        let mut mods: Vec<_> = raw.iter().map(|m| parse::parse_mod(&profile, m)).collect();
        let spec = profile.records.clone().unwrap();
        let scan = scan(&spec, &raw, &mods, None);
        scan.enrich(&mut mods);
        (scan, mods)
    }

    #[test]
    fn two_plugins_editing_the_same_record_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);
        mod_with_plugin(dir.path(), "ModB", "B.esp", &["Skyrim.esm"], &[0x0000_0ABC]);

        let (scan, _) = run(dir.path());
        let all: HashSet<&str> = ["ModA", "ModB"].into_iter().collect();
        let overlaps = scan.overlaps(&all);

        assert!(scan.unreadable.is_empty(), "{:?}", scan.unreadable);
        assert_eq!(overlaps.len(), 1);
        match &overlaps[0] {
            Conflict::RecordOverlap {
                plugins,
                mods,
                records,
                ..
            } => {
                assert_eq!(plugins.len(), 2);
                assert_eq!(mods, &["ModA".to_string(), "ModB".to_string()]);
                assert_eq!(*records, 1);
            }
            other => panic!("expected a record overlap, got {other:?}"),
        }
    }

    #[test]
    fn plugins_editing_different_records_do_not_conflict() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);
        mod_with_plugin(dir.path(), "ModB", "B.esp", &["Skyrim.esm"], &[0x0000_0FFF]);

        let (scan, _) = run(dir.path());
        let all: HashSet<&str> = ["ModA", "ModB"].into_iter().collect();

        assert!(scan.unreadable.is_empty(), "{:?}", scan.unreadable);
        assert!(scan.overlaps(&all).is_empty());
    }

    #[test]
    fn a_disabled_mod_takes_its_record_overlaps_with_it() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);
        mod_with_plugin(dir.path(), "ModB", "B.esp", &["Skyrim.esm"], &[0x0000_0ABC]);

        let (scan, _) = run(dir.path());
        let only_a: HashSet<&str> = ["ModA"].into_iter().collect();

        assert!(scan.overlaps(&only_a).is_empty());
    }

    #[test]
    fn the_games_own_masters_are_not_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);

        let (_, mods) = run(dir.path());

        // Skyrim.esm ships with the game, so it is never a missing dependency.
        assert!(mods[0].requires.is_empty());
    }

    #[test]
    fn a_master_from_another_mod_becomes_a_dependency() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(
            dir.path(),
            "Patch",
            "Patch.esp",
            &["Skyrim.esm", "BigMod.esp"],
            &[0x0100_0ABC],
        );

        let (_, mods) = run(dir.path());

        assert_eq!(mods[0].requires.len(), 1);
        assert_eq!(mods[0].requires[0].name, "BigMod.esp");
        assert_eq!(mods[0].requires[0].kind, DepKind::Required);
    }

    #[test]
    fn a_mod_provides_the_plugin_filenames_it_installs() {
        let dir = tempfile::tempdir().unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);

        let (_, mods) = run(dir.path());

        assert!(mods[0].provides.iter().any(|s| {
            s.kind == SymbolKind::PluginFile && s.name == "A.esp"
        }));
    }

    #[test]
    fn two_plugins_from_the_same_mod_never_conflict_with_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("ModA");
        std::fs::create_dir_all(&one).unwrap();
        write_plugin(&one.join("A.esp"), &["Skyrim.esm"], &[0x0000_0ABC]);
        write_plugin(&one.join("B.esp"), &["Skyrim.esm"], &[0x0000_0ABC]);

        let (scan, _) = run(dir.path());
        let all: HashSet<&str> = ["ModA"].into_iter().collect();

        assert!(scan.overlaps(&all).is_empty());
    }

    #[test]
    fn a_malformed_plugin_is_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("BrokenMod");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("Broken.esp"), b"not a plugin at all").unwrap();
        mod_with_plugin(dir.path(), "ModA", "A.esp", &["Skyrim.esm"], &[0x0000_0ABC]);

        let (scan, mods) = run(dir.path());

        assert_eq!(scan.unreadable.len(), 1);
        assert!(scan.unreadable[0].contains("Broken.esp"));
        // The mod still exists, and still claims the plugin filename.
        assert_eq!(mods.len(), 2);
        assert_eq!(scan.plugin_count(), 2);
    }
}
