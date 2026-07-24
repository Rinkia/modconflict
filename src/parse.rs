//! Applies a profile to a scanned mod: `Profile + RawMod -> ModEntry`.
//!
//! This is the only place that turns game-specific shapes into the shared
//! model, and it is driven entirely by data — there is no per-game code.

use crate::model::{Dep, DepKind, ModEntry, Symbol, SymbolKind};
use crate::profile::{DeclaredKind, DepSyntax, DependencySource, MetadataReader, Profile};
use crate::scan::RawMod;
use crate::value::{self, Value};

pub fn parse_mod(profile: &Profile, raw: &RawMod) -> ModEntry {
    // No metadata file (Creation Engine) means nothing to parse: the id comes
    // from the filename and the value is in the file inventory, not the fields.
    let doc = match (profile.metadata_file.as_deref(), profile.format) {
        (Some(name), Some(format)) => raw
            .metadata_named(name)
            .and_then(|bytes| value::load(bytes, format).ok()),
        _ => None,
    };

    // A mod whose metadata is missing or malformed still exists on disk and can
    // still collide, so fall back to the filename rather than dropping it.
    let root = doc.as_ref().and_then(|d| {
        if profile.root.is_empty() {
            Some(d)
        } else {
            d.get(&profile.root)
        }
    });

    // A code-backed reader supplies what a text path cannot reach. It answers
    // in the same shape, so everything downstream is unchanged.
    let extracted = profile
        .metadata_reader
        .map(|reader| run_reader(reader, raw))
        .unwrap_or_default();

    let id = root
        .and_then(|d| profile.id_field.as_deref().and_then(|f| d.get_str(f)))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(extracted.id)
        .unwrap_or_else(|| id_from_filename(&raw.source_name()));

    let version = root
        .and_then(|d| profile.version_field.as_deref().and_then(|f| d.get_str(f)))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(extracted.version);

    let mut provides = vec![Symbol {
        kind: SymbolKind::ModId,
        name: id.clone(),
    }];
    if let (Some(doc), Some(field)) = (root, profile.provides_field.as_deref()) {
        for value in doc.get(field).map(Value::items).unwrap_or_default() {
            if let Some(name) = value.as_str().filter(|s| !s.is_empty()) {
                provides.push(Symbol {
                    kind: SymbolKind::ModId,
                    name: name.to_string(),
                });
            }
        }
    }

    let mut requires: Vec<Dep> = root
        .map(|doc| {
            profile
                .dependency_sources
                .iter()
                .flat_map(|source| read_dependencies(doc, source))
                .collect()
        })
        .unwrap_or_default();
    requires.extend(extracted.requires);

    ModEntry {
        id,
        version,
        files: raw.files.clone(),
        provides,
        requires,
    }
}

/// A reader failing is not fatal: the mod still exists on disk, keeps its
/// filename-derived id, and still takes part in file overlap.
fn run_reader(reader: MetadataReader, raw: &RawMod) -> crate::bg3::Extracted {
    // These readers open the mod as a single file. A folder-shaped mod simply
    // has nothing for them to read, which is not worth a warning.
    if !raw.source.is_file() {
        return Default::default();
    }
    match reader {
        MetadataReader::Bg3Pak => crate::bg3::read(&raw.source).unwrap_or_else(|e| {
            eprintln!("warning: {e:#}");
            Default::default()
        }),
    }
}

fn read_dependencies(doc: &Value, source: &DependencySource) -> Vec<Dep> {
    match source.syntax {
        DepSyntax::PrefixedStrings => doc
            .get_all(&source.field)
            .into_iter()
            .flat_map(Value::items)
            .filter_map(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|raw| parse_prefixed_dep(raw, source.kind.into()))
            .collect(),

        DepSyntax::Map => doc
            .get_all(&source.field)
            .into_iter()
            .filter_map(|v| match v {
                Value::Map(m) => Some(m),
                _ => None,
            })
            .flat_map(|m| m.iter())
            .map(|(name, req)| Dep {
                name: name.clone(),
                req: normalize_req(req.as_str(), source),
                kind: source.kind.into(),
            })
            .collect(),

        DepSyntax::Tables => doc
            .get_all(&source.field)
            .into_iter()
            .flat_map(Value::items)
            .filter_map(|table| read_dep_table(table, source))
            .collect(),
    }
}

fn read_dep_table(table: &Value, source: &DependencySource) -> Option<Dep> {
    let name = table.get_str(source.name_field.as_deref()?)?;
    let req = source
        .version_field
        .as_deref()
        .and_then(|f| table.get_str(f))
        .and_then(|r| normalize_req(Some(r), source));

    // Either spelling of "this one is optional" downgrades the entry.
    let required = source.required_field.as_deref().and_then(|f| table.get_str(f));
    let optional = source.optional_field.as_deref().and_then(|f| table.get_str(f));
    let kind = match (required, optional) {
        (Some("false"), _) | (_, Some("true")) => DepKind::Optional,
        _ => source.kind.into(),
    };

    Some(Dep {
        name: name.to_string(),
        req,
        kind,
    })
}

/// `*` and empty strings mean "any version" — carrying them as a requirement
/// would only invite a pointless parse later.
///
/// `version_prefix` turns a bare version into a requirement. Without it a
/// minimum-version field reads as an exact-ish match and rejects every later
/// major version.
fn normalize_req(raw: Option<&str>, source: &DependencySource) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return None;
    }
    match source.version_prefix.as_deref() {
        // Already a requirement: leave it alone.
        Some(prefix) if !trimmed.starts_with(['>', '<', '=', '^', '~', '[', '(']) => {
            Some(format!("{prefix}{trimmed}"))
        }
        _ => Some(trimmed.to_string()),
    }
}

impl From<DeclaredKind> for DepKind {
    fn from(kind: DeclaredKind) -> DepKind {
        match kind {
            DeclaredKind::Required => DepKind::Required,
            DeclaredKind::Optional => DepKind::Optional,
            DeclaredKind::Incompatible => DepKind::Incompatible,
        }
    }
}

/// Dependency strings in the Factorio family:
///   `base >= 1.1.0`   required
///   `? optional-mod`  optional
///   `(?) hidden`      optional, hidden in the GUI
///   `! incompatible`  must not be installed
///   `~ no-load-order` required, does not affect load order
///
/// A plain name with no prefix takes `default_kind`: for Farming Simulator that
/// is a required dependency, for RimWorld's `incompatibleWith` list the very
/// same syntax means the opposite.
fn parse_prefixed_dep(raw: &str, default_kind: DepKind) -> Dep {
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
        (default_kind, text)
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

/// `boblogistics_1.2.3.zip` -> `boblogistics`
fn id_from_filename(name: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((base, ext)) if matches!(ext.to_ascii_lowercase().as_str(), "zip" | "jar") => base,
        _ => name,
    };

    // Split at the last `_` only when what follows looks like a version.
    match stem.rsplit_once('_') {
        Some((base, tail)) if tail.starts_with(|c: char| c.is_ascii_digit()) => base.to_string(),
        _ => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{by_name, load_all};
    use crate::testutil::raw_mod;

    fn profile(name: &str) -> Profile {
        by_name(&load_all(None).unwrap(), name).unwrap().clone()
    }

    #[test]
    fn factorio_reads_name_version_and_every_dependency_prefix() {
        let raw = raw_mod(
            "boblogistics_1.2.3.zip",
            &[(
                "boblogistics_1.2.3/info.json",
                r#"{"name":"boblogistics","version":"1.2.3",
                    "dependencies":["base >= 1.1.0","? bobplates","! angelsrefining","~ bobores"]}"#,
            )],
        );

        let entry = parse_mod(&profile("factorio"), &raw);

        assert_eq!(entry.id, "boblogistics");
        assert_eq!(entry.version.as_deref(), Some("1.2.3"));
        assert_eq!(entry.requires.len(), 4);
        assert_eq!(entry.requires[0].name, "base");
        assert_eq!(entry.requires[0].req.as_deref(), Some(">=1.1.0"));
        assert_eq!(entry.requires[0].kind, DepKind::Required);
        assert_eq!(entry.requires[1].kind, DepKind::Optional);
        assert_eq!(entry.requires[2].kind, DepKind::Incompatible);
        assert_eq!(entry.requires[3].kind, DepKind::Required);
    }

    #[test]
    fn fabric_reads_maps_and_splits_them_by_kind() {
        let raw = raw_mod(
            "coolmod.jar",
            &[(
                "fabric.mod.json",
                r#"{"id":"coolmod","version":"2.0.0",
                    "provides":["coolmod-api"],
                    "depends":{"fabricloader":">=0.14.0","minecraft":"*"},
                    "breaks":{"badmod":"*"}}"#,
            )],
        );

        let entry = parse_mod(&profile("minecraft-fabric"), &raw);

        assert_eq!(entry.id, "coolmod");
        assert!(entry
            .provides
            .iter()
            .any(|s| s.name == "coolmod-api"));

        let loader = entry.requires.iter().find(|d| d.name == "fabricloader").unwrap();
        assert_eq!(loader.req.as_deref(), Some(">=0.14.0"));
        assert_eq!(loader.kind, DepKind::Required);

        // "*" is any version, so it carries no requirement.
        let mc = entry.requires.iter().find(|d| d.name == "minecraft").unwrap();
        assert_eq!(mc.req, None);

        let bad = entry.requires.iter().find(|d| d.name == "badmod").unwrap();
        assert_eq!(bad.kind, DepKind::Incompatible);
    }

    #[test]
    fn forge_reads_tables_under_a_key_it_does_not_know_in_advance() {
        let raw = raw_mod(
            "coolmod-1.0.0.jar",
            &[(
                "META-INF/mods.toml",
                r#"
[[mods]]
modId = "coolmod"
version = "1.0.0"

[[dependencies.coolmod]]
modId = "forge"
mandatory = true
versionRange = "[36,)"

[[dependencies.coolmod]]
modId = "jei"
mandatory = false
versionRange = "[9,)"
"#,
            )],
        );

        let entry = parse_mod(&profile("minecraft-forge"), &raw);

        assert_eq!(entry.id, "coolmod");
        assert_eq!(entry.version.as_deref(), Some("1.0.0"));
        assert_eq!(entry.requires.len(), 2);

        let forge = entry.requires.iter().find(|d| d.name == "forge").unwrap();
        assert_eq!(forge.kind, DepKind::Required);

        // mandatory = false downgrades the entry to optional.
        let jei = entry.requires.iter().find(|d| d.name == "jei").unwrap();
        assert_eq!(jei.kind, DepKind::Optional);
    }

    #[test]
    fn farming_simulator_reads_xml_and_takes_its_id_from_the_filename() {
        let raw = raw_mod(
            "FS22_CoolMod.zip",
            &[(
                "modDesc.xml",
                r#"<modDesc descVersion="60">
                     <version>1.0.0.0</version>
                     <dependencies>
                       <dependency>FS22_BaseMod</dependency>
                       <dependency>FS22_OtherMod</dependency>
                     </dependencies>
                   </modDesc>"#,
            )],
        );

        let entry = parse_mod(&profile("farming-simulator"), &raw);

        assert_eq!(entry.id, "FS22_CoolMod");
        assert_eq!(entry.version.as_deref(), Some("1.0.0.0"));
        assert_eq!(entry.requires.len(), 2);
        assert_eq!(entry.requires[0].name, "FS22_BaseMod");
        assert_eq!(entry.requires[0].kind, DepKind::Required);
    }

    #[test]
    fn a_mod_with_malformed_metadata_still_gets_an_id() {
        let raw = raw_mod("broken_1.0.0.zip", &[("broken_1.0.0/info.json", "{ not json")]);

        let entry = parse_mod(&profile("factorio"), &raw);

        assert_eq!(entry.id, "broken");
        assert!(entry.requires.is_empty());
    }

    #[test]
    fn a_mod_with_no_metadata_at_all_still_gets_an_id() {
        let raw = raw_mod("mystery_2.0.0.zip", &[("mystery_2.0.0/data.lua", "-- x")]);

        assert_eq!(parse_mod(&profile("factorio"), &raw).id, "mystery");
    }

    #[test]
    fn keeps_spaces_inside_mod_names() {
        let dep = parse_prefixed_dep("? Squeak Through >= 1.8", DepKind::Required);

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
    fn strips_the_version_suffix_from_a_filename_but_not_a_plain_underscore() {
        assert_eq!(id_from_filename("my_cool_mod.zip"), "my_cool_mod");
        assert_eq!(id_from_filename("my_cool_mod_1.0.0.zip"), "my_cool_mod");
        assert_eq!(id_from_filename("coolmod.jar"), "coolmod");
    }
}
