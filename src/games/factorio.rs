//! Factorio: every mod carries an `info.json` at the root of its archive.
//!
//! Format reference: <https://wiki.factorio.com/Tutorial:Mod_structure>

use serde::Deserialize;

use crate::model::{Dep, DepKind, ModEntry, Symbol, SymbolKind};
use crate::scan::RawMod;

#[derive(Debug, Deserialize)]
struct InfoJson {
    name: String,
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

pub fn parse(raw: &RawMod) -> ModEntry {
    let info = raw
        .metadata_named("info.json")
        .and_then(|bytes| serde_json::from_slice::<InfoJson>(bytes).ok());

    // A mod with no readable info.json still exists on disk and can still
    // collide, so fall back to the filename rather than dropping it.
    let (id, version, deps) = match info {
        Some(i) => (i.name, i.version, i.dependencies),
        None => (id_from_filename(&raw.source_name()), None, Vec::new()),
    };

    ModEntry {
        id: id.clone(),
        version,
        files: raw.files.clone(),
        // ponytail: mod id only. Prototype-name collisions live inside data.lua
        // and would need a Lua parser — add one if id-level checks prove too coarse.
        provides: vec![Symbol {
            kind: SymbolKind::ModId,
            name: id,
        }],
        requires: deps.iter().map(|d| parse_dep(d)).collect(),
    }
}

/// `boblogistics_1.2.3.zip` -> `boblogistics`
fn id_from_filename(name: &str) -> String {
    let stem = name
        .strip_suffix(".zip")
        .or_else(|| name.strip_suffix(".ZIP"))
        .unwrap_or(name);

    // Split at the last `_` only when what follows looks like a version.
    match stem.rsplit_once('_') {
        Some((base, tail)) if tail.starts_with(|c: char| c.is_ascii_digit()) => base.to_string(),
        _ => stem.to_string(),
    }
}

/// Dependency strings look like:
///   `base >= 1.1.0`   required
///   `? optional-mod`  optional
///   `(?) hidden`      optional, hidden in the GUI
///   `! incompatible`  must not be installed
///   `~ no-load-order` required, does not affect load order
fn parse_dep(raw: &str) -> Dep {
    let text = raw.trim();

    let (kind, rest) = if let Some(r) = text.strip_prefix("(?)") {
        (DepKind::Optional, r)
    } else if let Some(r) = text.strip_prefix('?') {
        (DepKind::Optional, r)
    } else if let Some(r) = text.strip_prefix('!') {
        (DepKind::Incompatible, r)
    } else if let Some(r) = text.strip_prefix('~') {
        (DepKind::Required, r)
    } else {
        (DepKind::Required, text)
    };

    let (name, req) = split_version_req(rest.trim());
    Dep { name, req, kind }
}

/// Mod names may contain spaces, so split on the comparator rather than on
/// whitespace. Longest operators first so `>=` never matches as `>`.
fn split_version_req(text: &str) -> (String, Option<String>) {
    const OPERATORS: &[&str] = &[">=", "<=", "==", ">", "<", "="];

    for op in OPERATORS {
        if let Some(pos) = text.find(op) {
            let name = text[..pos].trim().to_string();
            let version = text[pos + op.len()..].trim();
            // semver has no `==`; normalize it to `=`.
            let op = if *op == "==" { "=" } else { op };
            return (name, Some(format!("{op}{version}")));
        }
    }
    (text.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::raw_mod;

    #[test]
    fn reads_name_version_and_dependencies_from_info_json() {
        let raw = raw_mod(
            "boblogistics_1.2.3.zip",
            &[(
                "boblogistics_1.2.3/info.json",
                r#"{"name":"boblogistics","version":"1.2.3",
                    "dependencies":["base >= 1.1.0","? bobplates","! angelsrefining"]}"#,
            )],
        );

        let entry = parse(&raw);

        assert_eq!(entry.id, "boblogistics");
        assert_eq!(entry.version.as_deref(), Some("1.2.3"));
        assert_eq!(entry.requires.len(), 3);
        assert_eq!(entry.requires[0].name, "base");
        assert_eq!(entry.requires[0].req.as_deref(), Some(">=1.1.0"));
        assert_eq!(entry.requires[0].kind, DepKind::Required);
        assert_eq!(entry.requires[1].kind, DepKind::Optional);
        assert_eq!(entry.requires[2].kind, DepKind::Incompatible);
    }

    #[test]
    fn falls_back_to_the_filename_when_info_json_is_missing() {
        let raw = raw_mod("mystery_2.0.0.zip", &[("mystery_2.0.0/data.lua", "-- x")]);

        let entry = parse(&raw);

        assert_eq!(entry.id, "mystery");
        assert_eq!(entry.version, None);
    }

    #[test]
    fn falls_back_to_the_filename_when_info_json_is_malformed() {
        let raw = raw_mod("broken_1.0.0.zip", &[("broken_1.0.0/info.json", "{ not json")]);

        assert_eq!(parse(&raw).id, "broken");
    }

    #[test]
    fn parses_every_dependency_prefix() {
        assert_eq!(parse_dep("base").kind, DepKind::Required);
        assert_eq!(parse_dep("~ base").kind, DepKind::Required);
        assert_eq!(parse_dep("? base").kind, DepKind::Optional);
        assert_eq!(parse_dep("(?) base").kind, DepKind::Optional);
        assert_eq!(parse_dep("! base").kind, DepKind::Incompatible);
    }

    #[test]
    fn keeps_spaces_inside_mod_names() {
        let dep = parse_dep("? Squeak Through >= 1.8");

        assert_eq!(dep.name, "Squeak Through");
        assert_eq!(dep.req.as_deref(), Some(">=1.8"));
        assert_eq!(dep.kind, DepKind::Optional);
    }

    #[test]
    fn does_not_read_the_ge_operator_as_two_operators() {
        assert_eq!(split_version_req("base >= 1.1.0").1.as_deref(), Some(">=1.1.0"));
        assert_eq!(split_version_req("base > 1.1.0").1.as_deref(), Some(">1.1.0"));
    }

    #[test]
    fn a_dependency_without_a_version_has_no_requirement() {
        assert_eq!(parse_dep("base").req, None);
    }

    #[test]
    fn keeps_the_whole_name_when_the_filename_has_no_version_suffix() {
        assert_eq!(id_from_filename("my_cool_mod.zip"), "my_cool_mod");
        assert_eq!(id_from_filename("my_cool_mod_1.0.0.zip"), "my_cool_mod");
    }
}
