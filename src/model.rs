//! The game-agnostic model. Every game parser fills these types; the detector
//! reads only these types and never knows which game it is looking at.

use serde::Serialize;

pub type ModId = String;

/// One installed mod: an archive or an extracted folder.
#[derive(Debug, Clone)]
pub struct ModEntry {
    pub id: ModId,
    pub version: Option<String>,
    /// Paths inside the mod, normalized to forward slashes.
    pub files: Vec<String>,
    /// Identifiers this mod claims to own.
    pub provides: Vec<Symbol>,
    /// Dependencies the mod declares.
    pub requires: Vec<Dep>,
    /// Whether the profile found and understood this mod's metadata, as
    /// opposed to falling back to the filename. A folder where this is false
    /// everywhere means the profile is wrong, not that the mods are clean.
    pub metadata_found: bool,
}

/// A named thing a mod owns. Today only mod ids; prototype names (Factorio)
/// and FormIDs (Skyrim) become new `SymbolKind` variants, and the detector
/// keeps working unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    ModId,
    /// A plugin file a mod installs, e.g. `MyMod.esp`. Two mods installing the
    /// same plugin filename is a genuine clash: only one file survives on disk.
    PluginFile,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::ModId => f.write_str("mod id"),
            SymbolKind::PluginFile => f.write_str("plugin"),
        }
    }
}

/// A declared dependency. `req` is a version requirement in semver syntax
/// (Factorio's `>= 1.1.0` parses directly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dep {
    pub name: String,
    pub req: Option<String>,
    pub kind: DepKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    /// Must be present and satisfy `req`.
    Required,
    /// Only checked when present.
    Optional,
    /// Must NOT be present.
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Severity::Info => "INFO",
            Severity::Warning => "WARN",
            Severity::Critical => "CRIT",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Conflict {
    /// Two or more mods ship the same internal path — last one loaded wins.
    /// `winner` is filled in when a load order is available.
    FileOverlap {
        path: String,
        mods: Vec<ModId>,
        winner: Option<ModId>,
    },
    /// Two or more mods claim the same identifier.
    DuplicateId { symbol: Symbol, mods: Vec<ModId> },
    /// A required dependency is not installed.
    MissingDep { mod_id: ModId, dep: Dep },
    /// The dependency is installed but its version does not satisfy the requirement.
    VersionMismatch {
        mod_id: ModId,
        dep: Dep,
        found: String,
    },
    /// A mod declares another installed mod as incompatible.
    Incompatible { mod_id: ModId, other: ModId },
    /// Two plugins edit the same records. Normal and often deliberate — it is
    /// how compatibility patches work — but the later plugin's version of every
    /// shared record is the one that survives.
    RecordOverlap {
        plugins: Vec<String>,
        mods: Vec<ModId>,
        records: usize,
    },
}

impl Conflict {
    pub fn severity(&self) -> Severity {
        match self {
            // File overlap is often deliberate (compatibility patches), so it
            // never blocks — it only warns.
            Conflict::FileOverlap { .. } | Conflict::RecordOverlap { .. } => Severity::Warning,
            Conflict::DuplicateId { .. }
            | Conflict::MissingDep { .. }
            | Conflict::VersionMismatch { .. }
            | Conflict::Incompatible { .. } => Severity::Critical,
        }
    }

    /// One-line label for list views.
    pub fn title(&self) -> String {
        match self {
            Conflict::FileOverlap { path, mods, winner } => match winner {
                Some(w) => format!("{} mods ship {} ({w} wins)", mods.len(), path),
                None => format!("{} mods ship {}", mods.len(), path),
            },
            Conflict::DuplicateId { symbol, mods } => {
                format!("{} {} claimed by {} mods", symbol.kind, symbol.name, mods.len())
            }
            Conflict::MissingDep { mod_id, dep } => {
                format!("{mod_id} needs missing {}", dep.name)
            }
            Conflict::VersionMismatch { mod_id, dep, .. } => {
                format!("{mod_id} needs {} {}", dep.name, dep.req.as_deref().unwrap_or(""))
            }
            Conflict::Incompatible { mod_id, other } => {
                format!("{mod_id} is incompatible with {other}")
            }
            Conflict::RecordOverlap {
                plugins, records, ..
            } => format!(
                "{} and {} both edit {records} records",
                plugins.first().map(String::as_str).unwrap_or("?"),
                plugins.get(1).map(String::as_str).unwrap_or("?")
            ),
        }
    }

    /// Multi-line explanation with the suggested fix.
    pub fn detail(&self) -> String {
        match self {
            Conflict::FileOverlap { path, mods, winner } => format!(
                "Path: {path}\n\nProvided by:\n{}\n\n{}",
                bullets(mods),
                match winner {
                    Some(w) => format!(
                        "\"{w}\" loads last, so its version of this file is the one the game \
                         uses. Every other copy above is silently ignored. If that is not what \
                         you want, change the load order."
                    ),
                    None => "Whichever mod loads last wins, and no load order file was found so \
                             the winner is unknown. If this is not a deliberate compatibility \
                             patch, expect the losing mod's version of this file to be silently \
                             ignored."
                        .to_string(),
                }
            ),
            Conflict::DuplicateId { symbol, mods } => format!(
                "{} \"{}\" is claimed by:\n{}\n\nThe game cannot tell these apart. Remove all but \
                 one, or rename the identifier.",
                symbol.kind,
                symbol.name,
                bullets(mods)
            ),
            Conflict::MissingDep { mod_id, dep } => format!(
                "{mod_id} requires \"{}\"{} but it is not installed.\n\nInstall it, or remove \
                 {mod_id}.",
                dep.name,
                dep.req
                    .as_deref()
                    .map(|r| format!(" {r}"))
                    .unwrap_or_default()
            ),
            Conflict::VersionMismatch { mod_id, dep, found } => format!(
                "{mod_id} requires \"{}\" {}, but version {found} is installed.\n\nUpdate \
                 \"{}\", or downgrade {mod_id}.",
                dep.name,
                dep.req.as_deref().unwrap_or(""),
                dep.name
            ),
            Conflict::Incompatible { mod_id, other } => format!(
                "{mod_id} declares \"{other}\" as incompatible, and {other} is installed.\n\n\
                 Remove one of the two."
            ),
            Conflict::RecordOverlap {
                plugins,
                mods,
                records,
            } => format!(
                "These two plugins edit {records} of the same records:\n{}\n\nFrom:\n{}\n\n\
                 Whichever plugin loads later overwrites the other for every shared record. If \
                 neither is a patch written for the other, load order alone decides which mod's \
                 changes you actually get, and a compatibility patch may be needed.",
                bullets(plugins),
                bullets(mods)
            ),
        }
    }

    /// Mods involved — used by the TUI filter.
    pub fn mods(&self) -> Vec<&str> {
        match self {
            Conflict::FileOverlap { mods, .. }
            | Conflict::DuplicateId { mods, .. }
            | Conflict::RecordOverlap { mods, .. } => mods.iter().map(String::as_str).collect(),
            Conflict::MissingDep { mod_id, dep } | Conflict::VersionMismatch { mod_id, dep, .. } => {
                vec![mod_id.as_str(), dep.name.as_str()]
            }
            Conflict::Incompatible { mod_id, other } => vec![mod_id.as_str(), other.as_str()],
        }
    }
}

fn bullets(items: &[String]) -> String {
    items
        .iter()
        .map(|m| format!("  - {m}"))
        .collect::<Vec<_>>()
        .join("\n")
}
