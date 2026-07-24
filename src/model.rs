//! The game-agnostic model. Every game parser fills these types; the detector
//! reads only these types and never knows which game it is looking at.

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
}

/// A named thing a mod owns. Today only mod ids; prototype names (Factorio)
/// and FormIDs (Skyrim) become new `SymbolKind` variants, and the detector
/// keeps working unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    ModId,
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::ModId => f.write_str("mod id"),
        }
    }
}

/// A declared dependency. `req` is a version requirement in semver syntax
/// (Factorio's `>= 1.1.0` parses directly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub req: Option<String>,
    pub kind: DepKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    /// Must be present and satisfy `req`.
    Required,
    /// Only checked when present.
    Optional,
    /// Must NOT be present.
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two or more mods ship the same internal path — last one loaded wins.
    FileOverlap { path: String, mods: Vec<ModId> },
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
}

impl Conflict {
    pub fn severity(&self) -> Severity {
        match self {
            // File overlap is often deliberate (compatibility patches), so it
            // never blocks — it only warns.
            Conflict::FileOverlap { .. } => Severity::Warning,
            Conflict::DuplicateId { .. }
            | Conflict::MissingDep { .. }
            | Conflict::VersionMismatch { .. }
            | Conflict::Incompatible { .. } => Severity::Critical,
        }
    }

    /// One-line label for list views.
    pub fn title(&self) -> String {
        match self {
            Conflict::FileOverlap { path, mods } => {
                format!("{} mods ship {}", mods.len(), path)
            }
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
        }
    }

    /// Multi-line explanation with the suggested fix.
    pub fn detail(&self) -> String {
        match self {
            Conflict::FileOverlap { path, mods } => format!(
                "Path: {path}\n\nProvided by:\n{}\n\nWhichever mod loads last wins. If this is \
                 not a deliberate compatibility patch, expect the losing mod's version of this \
                 file to be silently ignored.",
                bullets(mods)
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
        }
    }

    /// Mods involved — used by the TUI filter.
    pub fn mods(&self) -> Vec<&str> {
        match self {
            Conflict::FileOverlap { mods, .. } | Conflict::DuplicateId { mods, .. } => {
                mods.iter().map(String::as_str).collect()
            }
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
