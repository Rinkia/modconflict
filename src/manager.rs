//! Mod managers.
//!
//! The Creation Engine profile deliberately ships without a load order,
//! because `plugins.txt` lists *plugin* names while a mod id here is the mod
//! folder name, and guessing the mapping would name the wrong winner with total
//! confidence. A mod manager knows both, so this module reads what it wrote.
//!
//! The mapping itself still does not need reading: the scan already knows which
//! mod ships which plugin. What is missing is only the two orderings — which
//! mod overwrites which, and which plugin loads after which — and those are
//! plain text files.
//!
//! Read-only, always. The manager's own files are never written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::loadorder::LoadOrder;

/// Manager files are edited by hand and by Windows tools, so a UTF-8 BOM on
/// the first line is common. Left in place it becomes part of the first mod's
/// name — and indexing past it by byte panics outright.
fn lines_without_bom(text: &str) -> impl Iterator<Item = &str> {
    text.trim_start_matches('\u{feff}').lines().map(str::trim)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[value(rename_all = "lower")]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    /// Mod Organizer 2, whose profile files are plain text.
    Mo2,
    /// Do not look for a manager at all.
    None,
}

#[derive(Debug, Clone)]
pub struct ManagerData {
    pub name: &'static str,
    /// Which of the manager's profiles was read.
    pub profile: String,
    /// Mods in load order, earliest first, plus the ones switched off.
    pub mod_order: LoadOrder,
    /// Plugin filenames in load order, earliest first.
    #[cfg_attr(not(feature = "records"), allow(dead_code))]
    pub plugin_order: Vec<String>,
}

impl ManagerData {
    /// Which of these plugins wins a record overlap: the one loaded last.
    #[cfg_attr(not(feature = "records"), allow(dead_code))]
    pub fn plugin_winner<'a>(&self, plugins: &'a [String]) -> Option<&'a String> {
        plugins
            .iter()
            .filter_map(|p| {
                self.plugin_order
                    .iter()
                    .position(|known| known.eq_ignore_ascii_case(p))
                    .map(|pos| (pos, p))
            })
            .max_by_key(|(pos, _)| *pos)
            .map(|(_, p)| p)
    }
}

/// Find and read whatever manager governs `mods_dir`.
///
/// `Ok(None)` means no manager was found, which is the ordinary case for
/// someone pointing at a bare game folder.
pub fn read(mods_dir: &Path, forced: Option<Manager>) -> Result<Option<ManagerData>> {
    match forced {
        Some(Manager::None) => Ok(None),
        Some(Manager::Mo2) => {
            let root = mo2_root(mods_dir).ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: no Mod Organizer 2 profile found. Point at the instance's \
                     mods/ folder, which must have a sibling profiles/ folder.",
                    mods_dir.display()
                )
            })?;
            read_mo2(&root).map(Some)
        }
        None => match mo2_root(mods_dir) {
            Some(root) => read_mo2(&root).map(Some),
            None => Ok(None),
        },
    }
}

/// An MO2 instance keeps `mods/` and `profiles/` side by side.
fn mo2_root(mods_dir: &Path) -> Option<PathBuf> {
    let parent = mods_dir.parent()?;
    parent
        .join("profiles")
        .is_dir()
        .then(|| parent.to_path_buf())
}

fn read_mo2(root: &Path) -> Result<ManagerData> {
    let profile = selected_profile(root)?;
    let dir = root.join("profiles").join(&profile);

    let mod_order = read_modlist(&dir.join("modlist.txt"))?;
    // loadorder.txt, where present, is the authoritative order; plugins.txt
    // carries the enabled flags and is the only file in newer setups.
    let plugin_order = match read_plugin_list(&dir.join("loadorder.txt"))? {
        Some(order) if !order.is_empty() => order,
        _ => read_plugin_list(&dir.join("plugins.txt"))?.unwrap_or_default(),
    };

    Ok(ManagerData {
        name: "Mod Organizer 2",
        profile,
        mod_order,
        plugin_order,
    })
}

/// `ModOrganizer.ini` names the active profile. Without it, fall back to the
/// only profile there is, then to MO2's own default name.
fn selected_profile(root: &Path) -> Result<String> {
    let ini = root.join("ModOrganizer.ini");
    if let Ok(text) = std::fs::read_to_string(&ini) {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("selected_profile=") {
                let name = value.trim().trim_matches('"');
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }

    let profiles: Vec<_> = std::fs::read_dir(root.join("profiles"))
        .with_context(|| format!("cannot read {}", root.join("profiles").display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    match profiles.as_slice() {
        [only] => Ok(only.clone()),
        _ => Ok("Default".to_string()),
    }
}

/// `modlist.txt`: `+Enabled`, `-Disabled`, and separators that are not mods.
///
/// The file is written **highest priority first** — the top line is the mod
/// that overwrites everything below it. Load order is therefore the file
/// reversed, and getting this backwards would name the losing mod as the
/// winner of every single overlap.
fn read_modlist(path: &Path) -> Result<LoadOrder> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(LoadOrder::default());
    };

    let mut enabled = Vec::new();
    let mut disabled = HashSet::new();

    for line in lines_without_bom(&text) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // strip_prefix, never split_at: a byte index into text this tool did
        // not write is a panic waiting for the first non-ASCII character.
        let (is_enabled, name) = if let Some(rest) = line.strip_prefix('+') {
            (true, rest)
        } else if let Some(rest) = line.strip_prefix('-') {
            (false, rest)
        } else {
            continue;
        };
        // Separators are UI furniture, not mods.
        if name.ends_with("_separator") {
            continue;
        }
        if is_enabled {
            enabled.push(name.to_string());
        } else {
            disabled.insert(name.to_string());
        }
    }

    enabled.reverse();
    Ok(LoadOrder {
        order: enabled,
        disabled,
    })
}

/// `plugins.txt` marks enabled plugins with `*`; `loadorder.txt` is a plain
/// list. Both are read the same way, keeping only what is turned on.
fn read_plugin_list(path: &Path) -> Result<Option<Vec<String>>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };

    let mut order = Vec::new();
    for line in lines_without_bom(&text) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_prefix('*') {
            Some(name) => order.push(name.trim().to_string()),
            // A plugins.txt line without the marker is disabled; a
            // loadorder.txt line has no marker at all and counts.
            None => order.push(line.to_string()),
        }
    }
    Ok(Some(order))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an MO2 instance: mods/ and profiles/<name>/ side by side.
    fn instance(profile: &str, modlist: &str, plugins: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        let profile_dir = dir.path().join("profiles").join(profile);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("modlist.txt"), modlist).unwrap();
        if let Some(plugins) = plugins {
            std::fs::write(profile_dir.join("plugins.txt"), plugins).unwrap();
        }
        std::fs::write(
            dir.path().join("ModOrganizer.ini"),
            format!("[General]\nselected_profile={profile}\n"),
        )
        .unwrap();
        dir
    }

    #[test]
    fn an_mo2_instance_is_found_from_its_mods_folder() {
        let dir = instance("Default", "+Alpha\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.name, "Mod Organizer 2");
        assert_eq!(data.profile, "Default");
    }

    /// PowerShell writes a BOM by default, so this is what a hand-edited
    /// modlist.txt actually looks like on Windows. Indexing past it by byte
    /// used to panic outright.
    #[test]
    fn a_byte_order_mark_does_not_crash_or_corrupt_the_first_mod() {
        let dir = instance("Default", "\u{feff}+Alpha\n+Beta\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.mod_order.order, vec!["Beta", "Alpha"]);
    }

    #[test]
    fn a_non_ascii_mod_name_survives_intact() {
        let dir = instance("Default", "+\u{c9}p\u{e9}es\n+\u{65e5}\u{672c}Mod\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(
            data.mod_order.order,
            vec!["\u{65e5}\u{672c}Mod", "\u{c9}p\u{e9}es"]
        );
    }

    #[test]
    fn a_plain_folder_has_no_manager_and_that_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();

        assert!(read(dir.path(), None).unwrap().is_none());
    }

    #[test]
    fn asking_for_mo2_where_there_is_none_is_an_error() {
        let dir = tempfile::tempdir().unwrap();

        assert!(read(dir.path(), Some(Manager::Mo2)).is_err());
    }

    #[test]
    fn none_skips_a_manager_that_is_right_there() {
        let dir = instance("Default", "+Alpha\n", None);

        assert!(read(&dir.path().join("mods"), Some(Manager::None))
            .unwrap()
            .is_none());
    }

    /// The detail that decides every overlap: MO2 writes the winner first.
    #[test]
    fn modlist_priority_is_reversed_into_load_order() {
        let dir = instance("Default", "+Winner\n+Middle\n+Loser\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.mod_order.order, vec!["Loser", "Middle", "Winner"]);
        let mods = ["Loser".to_string(), "Winner".to_string()];
        assert_eq!(
            data.mod_order.winner(&mods).map(String::as_str),
            Some("Winner")
        );
    }

    #[test]
    fn a_minus_prefix_marks_a_mod_as_switched_off() {
        let dir = instance("Default", "+On\n-Off\n+AlsoOn\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert!(data.mod_order.is_disabled("Off"));
        assert!(!data.mod_order.is_disabled("On"));
        assert_eq!(data.mod_order.order, vec!["AlsoOn", "On"]);
    }

    #[test]
    fn separators_are_not_mods() {
        let dir = instance("Default", "+Alpha\n-Gameplay_separator\n+Beta\n", None);

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.mod_order.order, vec!["Beta", "Alpha"]);
        assert!(!data.mod_order.is_disabled("Gameplay_separator"));
    }

    #[test]
    fn the_plugin_list_gives_the_winner_of_a_record_overlap() {
        let dir = instance(
            "Default",
            "+Alpha\n",
            Some("*Skyrim.esm\n*First.esp\n*Second.esp\n"),
        );

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.plugin_order.len(), 3);
        let plugins = ["Second.esp".to_string(), "First.esp".to_string()];
        assert_eq!(
            data.plugin_winner(&plugins).map(String::as_str),
            Some("Second.esp")
        );
    }

    #[test]
    fn plugin_names_are_matched_regardless_of_case() {
        let dir = instance("Default", "+Alpha\n", Some("*First.esp\n*SECOND.ESP\n"));

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        let plugins = ["second.esp".to_string(), "First.esp".to_string()];
        assert_eq!(
            data.plugin_winner(&plugins).map(String::as_str),
            Some("second.esp")
        );
    }

    #[test]
    fn a_plugin_the_order_never_heard_of_has_no_winner() {
        let dir = instance("Default", "+Alpha\n", Some("*Known.esp\n"));

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        let plugins = ["Unknown.esp".to_string(), "AlsoUnknown.esp".to_string()];
        assert_eq!(data.plugin_winner(&plugins), None);
    }

    #[test]
    fn loadorder_txt_wins_over_plugins_txt_when_both_exist() {
        let dir = instance("Default", "+Alpha\n", Some("*B.esp\n*A.esp\n"));
        std::fs::write(
            dir.path().join("profiles/Default/loadorder.txt"),
            "A.esp\nB.esp\n",
        )
        .unwrap();

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.plugin_order, vec!["A.esp", "B.esp"]);
    }

    #[test]
    fn the_only_profile_is_used_when_the_ini_says_nothing() {
        let dir = instance("OnlyOne", "+Alpha\n", None);
        std::fs::remove_file(dir.path().join("ModOrganizer.ini")).unwrap();

        let data = read(&dir.path().join("mods"), None).unwrap().unwrap();

        assert_eq!(data.profile, "OnlyOne");
    }
}
