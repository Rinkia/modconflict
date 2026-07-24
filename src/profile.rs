//! A game profile: a data file describing where a game keeps its mod metadata
//! and how to read it. Adding support for a game means writing one TOML file —
//! no Rust, no rebuild if the file is dropped into the user profile directory.
//!
//! This is what makes the tool general. Almost every modern game ships mod
//! metadata as JSON, TOML, XML or INI inside the mod archive; the differences
//! are field names and dependency syntax, which is exactly what a profile
//! captures.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::scan::RawMod;
use crate::value::Format;

/// Profiles compiled into the binary. Users can add their own without
/// recompiling; see `load_dir`.
const BUILTIN: &[(&str, &str)] = &[
    ("factorio", include_str!("../profiles/factorio.toml")),
    (
        "minecraft-fabric",
        include_str!("../profiles/minecraft-fabric.toml"),
    ),
    (
        "minecraft-forge",
        include_str!("../profiles/minecraft-forge.toml"),
    ),
    (
        "farming-simulator",
        include_str!("../profiles/farming-simulator.toml"),
    ),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Machine name, used for `--game`.
    pub name: String,
    pub display_name: String,
    /// Metadata filename to look for inside each mod, matched on the basename
    /// so it is found at any depth.
    pub metadata_file: String,
    pub format: Format,
    /// Path prefix into the parsed document, e.g. `modDesc` for XML.
    #[serde(default)]
    pub root: String,
    /// Path to the mod id. When absent or missing, the filename is used.
    #[serde(default)]
    pub id_field: Option<String>,
    #[serde(default)]
    pub version_field: Option<String>,
    /// Extra ids this mod claims to satisfy (Fabric's `provides`).
    #[serde(default)]
    pub provides_field: Option<String>,
    /// Off for games where each mod lives in its own namespace, so two mods
    /// shipping the same internal path is normal rather than a conflict.
    #[serde(default = "yes")]
    pub check_file_overlap: bool,
    #[serde(default, rename = "dependencies")]
    pub dependency_sources: Vec<DependencySource>,
    #[serde(default)]
    pub load_order: Option<LoadOrderSpec>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySource {
    /// Path to the dependency collection inside the document.
    pub field: String,
    pub syntax: DepSyntax,
    /// Kind for every entry in this collection. Ignored by `prefixed-strings`,
    /// which reads the kind from each entry's prefix.
    #[serde(default = "required")]
    pub kind: DeclaredKind,
    /// `tables` only: where the name and version requirement live.
    #[serde(default)]
    pub name_field: Option<String>,
    #[serde(default)]
    pub version_field: Option<String>,
    /// `tables` only: a boolean field where `false` means optional.
    #[serde(default)]
    pub required_field: Option<String>,
}

fn required() -> DeclaredKind {
    DeclaredKind::Required
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DepSyntax {
    /// A list of strings, optionally prefixed with `?`/`(?)`/`!`/`~` and
    /// carrying an inline version comparator: `base >= 1.1.0`.
    PrefixedStrings,
    /// A map of `name -> version requirement`.
    Map,
    /// A list of tables, each with a name field and a version field.
    Tables,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeclaredKind {
    Required,
    Optional,
    Incompatible,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadOrderSpec {
    /// Filename to look for in the mod folder, then in its parent.
    pub file: String,
    pub format: LoadOrderFormat,
    /// `json`/`toml` only: path to the list of entries.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub name_field: Option<String>,
    #[serde(default)]
    pub enabled_field: Option<String>,
    /// `lines` only: prefix marking an enabled entry, e.g. Skyrim's `*`.
    #[serde(default)]
    pub enabled_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoadOrderFormat {
    /// One entry per line, `#` comments ignored.
    Lines,
    Json,
    Toml,
}

/// Every profile available to this run: the built-ins, plus any user profile,
/// which wins on a name clash so a user can fix a stale built-in without
/// waiting for a release.
pub fn load_all(user_dir: Option<&Path>) -> Result<Vec<Profile>> {
    let mut profiles: Vec<Profile> = Vec::new();
    for (name, text) in BUILTIN {
        profiles.push(
            toml::from_str(text)
                .with_context(|| format!("built-in profile {name} is malformed"))?,
        );
    }

    if let Some(dir) = user_dir {
        for user in load_dir(dir)? {
            profiles.retain(|p| p.name != user.name);
            profiles.push(user);
        }
    }
    Ok(profiles)
}

/// Read every `.toml` in a directory as a profile. A missing directory is not
/// an error — most users never create one.
pub fn load_dir(dir: &Path) -> Result<Vec<Profile>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut profiles = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let profile: Profile = toml::from_str(&text)
            .with_context(|| format!("invalid profile {}", path.display()))?;
        profiles.push(profile);
    }
    Ok(profiles)
}

/// Pick the profile whose metadata file the most mods actually carry.
pub fn detect<'a>(profiles: &'a [Profile], raw: &[RawMod]) -> Result<&'a Profile> {
    let best = profiles
        .iter()
        .map(|p| {
            let hits = raw
                .iter()
                .filter(|m| m.metadata_named(&p.metadata_file).is_some())
                .count();
            (p, hits)
        })
        .max_by_key(|(_, hits)| *hits);

    match best {
        Some((profile, hits)) if hits > 0 => Ok(profile),
        _ => bail!(
            "could not tell which game this folder is for — no known metadata file found. \
             Pass --game explicitly. Known games: {}",
            profiles
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn by_name<'a>(profiles: &'a [Profile], name: &str) -> Result<&'a Profile> {
    profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .with_context(|| {
            format!(
                "unknown game \"{name}\". Known games: {}",
                profiles
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Every metadata filename any profile might look for — the scanner keeps the
/// contents of these and only these.
pub fn metadata_filenames(profiles: &[Profile]) -> BTreeSet<String> {
    profiles.iter().map(|p| p.metadata_file.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_profile_parses() {
        let profiles = load_all(None).unwrap();
        assert_eq!(profiles.len(), BUILTIN.len());
        assert!(profiles.iter().any(|p| p.name == "factorio"));
    }

    #[test]
    fn a_user_profile_overrides_a_builtin_of_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("factorio.toml"),
            r#"
name = "factorio"
display_name = "Factorio (patched)"
metadata_file = "info.json"
format = "json"
"#,
        )
        .unwrap();

        let profiles = load_all(Some(dir.path())).unwrap();

        assert_eq!(profiles.len(), BUILTIN.len());
        let factorio = by_name(&profiles, "factorio").unwrap();
        assert_eq!(factorio.display_name, "Factorio (patched)");
    }

    #[test]
    fn a_missing_user_directory_is_not_an_error() {
        assert!(load_all(Some(Path::new("no/such/dir"))).is_ok());
    }

    #[test]
    fn a_malformed_user_profile_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.toml"), "name = ").unwrap();

        let err = load_all(Some(dir.path())).unwrap_err();

        assert!(format!("{err:#}").contains("broken.toml"));
    }

    #[test]
    fn detection_picks_the_profile_that_matches_the_most_mods() {
        let profiles = load_all(None).unwrap();
        let raw = vec![
            crate::testutil::raw_mod("a.zip", &[("a/info.json", "{}")]),
            crate::testutil::raw_mod("b.zip", &[("b/info.json", "{}")]),
            crate::testutil::raw_mod("c.jar", &[("fabric.mod.json", "{}")]),
        ];

        assert_eq!(detect(&profiles, &raw).unwrap().name, "factorio");
    }

    #[test]
    fn detection_fails_when_nothing_matches() {
        let profiles = load_all(None).unwrap();
        let raw = vec![crate::testutil::raw_mod("a.zip", &[("a/readme.txt", "hi")])];

        assert!(detect(&profiles, &raw).is_err());
    }

    #[test]
    fn an_unknown_game_name_lists_the_known_ones() {
        let profiles = load_all(None).unwrap();

        let err = by_name(&profiles, "doom").unwrap_err();

        assert!(format!("{err:#}").contains("factorio"));
    }
}
