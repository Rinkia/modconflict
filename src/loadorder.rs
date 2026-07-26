//! Reads the game's load-order file, when the profile says it has one.
//!
//! Load order answers two questions the rest of the tool cannot: which mods are
//! actually turned on, and — when two mods ship the same file — which one wins.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::profile::{LoadOrderFormat, LoadOrderSpec, Profile};
use crate::value::{self, Format, Value};

#[derive(Debug, Default, Clone)]
pub struct LoadOrder {
    /// Mod ids in load order, earliest first.
    pub order: Vec<String>,
    /// Mod ids present in the file but switched off.
    pub disabled: HashSet<String>,
}

impl LoadOrder {
    /// Which of these mods wins a file overlap: the one loaded last. `None`
    /// when the order says nothing about any of them.
    pub fn winner<'a>(&self, mods: &'a [String]) -> Option<&'a String> {
        mods.iter()
            .filter_map(|m| self.position(m).map(|pos| (pos, m)))
            .max_by_key(|(pos, _)| *pos)
            .map(|(_, m)| m)
    }

    fn position(&self, mod_id: &str) -> Option<usize> {
        self.order.iter().position(|m| m == mod_id)
    }

    pub fn is_disabled(&self, mod_id: &str) -> bool {
        self.disabled.contains(mod_id)
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.disabled.is_empty()
    }
}

/// Find and read the load-order file. An explicit `override_path` always wins;
/// otherwise the profile's filename is looked for in the mod folder and then in
/// its parent, which is where most games keep it.
///
/// A missing file is not an error: plenty of installs have none, and the rest
/// of the report is still worth printing.
pub fn read(profile: &Profile, mods_dir: &Path, override_path: Option<&Path>) -> Result<LoadOrder> {
    let path = match (override_path, profile.load_order.as_ref()) {
        (Some(explicit), _) => explicit.to_path_buf(),
        (None, Some(spec)) => match find(mods_dir, &spec.file) {
            Some(found) => found,
            None => return Ok(LoadOrder::default()),
        },
        (None, None) => return Ok(LoadOrder::default()),
    };

    let Some(spec) = profile.load_order.as_ref() else {
        anyhow::bail!(
            "{} has no load-order format defined, so --load-order cannot be read for it",
            profile.display_name
        );
    };

    let bytes = std::fs::read(&path)
        .with_context(|| format!("cannot read load order file {}", path.display()))?;

    parse(&bytes, spec).with_context(|| format!("cannot parse {}", path.display()))
}

fn find(mods_dir: &Path, filename: &str) -> Option<PathBuf> {
    let here = mods_dir.join(filename);
    if here.is_file() {
        return Some(here);
    }
    let parent = mods_dir.parent()?.join(filename);
    parent.is_file().then_some(parent)
}

fn parse(bytes: &[u8], spec: &LoadOrderSpec) -> Result<LoadOrder> {
    match spec.format {
        LoadOrderFormat::Lines => Ok(parse_lines(bytes, spec)),
        LoadOrderFormat::Json => parse_structured(bytes, spec, Format::Json),
        LoadOrderFormat::Toml => parse_structured(bytes, spec, Format::Toml),
    }
}

/// One entry per line. When `enabled_prefix` is set (Skyrim's `plugins.txt`
/// uses `*`), a line without it is present but switched off.
fn parse_lines(bytes: &[u8], spec: &LoadOrderSpec) -> LoadOrder {
    let text = String::from_utf8_lossy(bytes);
    let mut load_order = LoadOrder::default();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let name = match spec.enabled_prefix.as_deref() {
            Some(prefix) => match line.strip_prefix(prefix) {
                Some(rest) => rest.trim().to_string(),
                None => {
                    load_order.disabled.insert(line.to_string());
                    continue;
                }
            },
            None => line.to_string(),
        };
        load_order.order.push(name);
    }
    load_order
}

fn parse_structured(bytes: &[u8], spec: &LoadOrderSpec, format: Format) -> Result<LoadOrder> {
    let doc = value::load(bytes, format)?;
    let entries = spec
        .path
        .as_deref()
        .and_then(|p| doc.get(p))
        .map(Value::items)
        .unwrap_or_default();

    let mut load_order = LoadOrder::default();
    for entry in entries {
        // Entries may be bare strings even when the format allows objects.
        let (name, enabled) = match (entry, spec.name_field.as_deref()) {
            (Value::Str(s), _) => (s.clone(), true),
            (map, Some(name_field)) => {
                let Some(name) = map.get_str(name_field) else {
                    continue;
                };
                let enabled = spec
                    .enabled_field
                    .as_deref()
                    .and_then(|f| map.get_str(f))
                    .map(|v| v != "false")
                    .unwrap_or(true);
                (name.to_string(), enabled)
            }
            _ => continue,
        };

        if enabled {
            load_order.order.push(name);
        } else {
            load_order.disabled.insert(name);
        }
    }
    Ok(load_order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{by_name, load_all};

    fn spec_lines() -> LoadOrderSpec {
        LoadOrderSpec {
            file: "plugins.txt".into(),
            format: LoadOrderFormat::Lines,
            path: None,
            name_field: None,
            enabled_field: None,
            enabled_prefix: Some("*".into()),
        }
    }

    fn factorio_spec() -> LoadOrderSpec {
        by_name(&load_all(None).unwrap(), "factorio")
            .unwrap()
            .load_order
            .clone()
            .unwrap()
    }

    #[test]
    fn reads_the_factorio_mod_list() {
        let json = br#"{"mods":[
            {"name":"base","enabled":true},
            {"name":"alpha","enabled":true},
            {"name":"turnedoff","enabled":false}
        ]}"#;

        let order = parse(json, &factorio_spec()).unwrap();

        assert_eq!(order.order, vec!["base", "alpha"]);
        assert!(order.is_disabled("turnedoff"));
        assert!(!order.is_disabled("alpha"));
    }

    #[test]
    fn reads_a_skyrim_style_plugin_list_where_a_star_means_enabled() {
        let text = b"# comment\n*Base.esm\n*Patch.esp\nDisabled.esp\n";

        let order = parse(text, &spec_lines()).unwrap();

        assert_eq!(order.order, vec!["Base.esm", "Patch.esp"]);
        assert!(order.is_disabled("Disabled.esp"));
    }

    #[test]
    fn every_line_counts_when_no_enabled_prefix_is_defined() {
        let mut spec = spec_lines();
        spec.enabled_prefix = None;

        let order = parse(b"Base.esm\nPatch.esp\n", &spec).unwrap();

        assert_eq!(order.order, vec!["Base.esm", "Patch.esp"]);
        assert!(order.disabled.is_empty());
    }

    #[test]
    fn the_mod_loaded_last_wins_an_overlap() {
        let order = LoadOrder {
            order: vec!["base".into(), "alpha".into(), "beta".into()],
            disabled: HashSet::new(),
        };

        let mods = ["alpha".to_string(), "beta".to_string()];
        let winner = order.winner(&mods);

        assert_eq!(winner.map(String::as_str), Some("beta"));
    }

    #[test]
    fn there_is_no_winner_when_the_order_knows_none_of_the_mods() {
        let order = LoadOrder {
            order: vec!["base".into()],
            disabled: HashSet::new(),
        };

        let mods = ["alpha".to_string(), "beta".to_string()];
        assert_eq!(order.winner(&mods), None);
    }

    #[test]
    fn a_mod_missing_from_the_order_never_beats_one_that_is_in_it() {
        let order = LoadOrder {
            order: vec!["alpha".into()],
            disabled: HashSet::new(),
        };

        let mods = ["alpha".to_string(), "unknown".to_string()];
        let winner = order.winner(&mods);

        assert_eq!(winner.map(String::as_str), Some("alpha"));
    }

    #[test]
    fn a_missing_load_order_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let profile = by_name(&load_all(None).unwrap(), "factorio")
            .unwrap()
            .clone();

        let order = read(&profile, dir.path(), None).unwrap();

        assert!(order.is_empty());
    }

    #[test]
    fn finds_the_load_order_file_in_the_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("mods");
        std::fs::create_dir(&mods).unwrap();
        std::fs::write(
            dir.path().join("mod-list.json"),
            r#"{"mods":[{"name":"base","enabled":true}]}"#,
        )
        .unwrap();
        let profile = by_name(&load_all(None).unwrap(), "factorio")
            .unwrap()
            .clone();

        let order = read(&profile, &mods, None).unwrap();

        assert_eq!(order.order, vec!["base"]);
    }

    #[test]
    fn a_profile_without_a_load_order_section_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let profile = by_name(&load_all(None).unwrap(), "farming-simulator")
            .unwrap()
            .clone();

        assert!(read(&profile, dir.path(), None).unwrap().is_empty());
    }
}
