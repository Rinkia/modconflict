//! Conflict detection. Pure function of `Vec<ModEntry>` — no I/O, no game
//! knowledge, fully testable without touching the filesystem.

use std::collections::{BTreeSet, HashMap};

use crate::loadorder::LoadOrder;
use crate::model::{Conflict, Dep, DepKind, ModEntry, Symbol};
use crate::versionreq::{self, Outcome};

/// Paths that collide in practically every mod and never mean anything.
///
/// Every profile's own metadata filename is added to this at runtime — a file
/// each mod of a game is *required* to ship is boring by definition, and
/// hardcoding the list here would go stale with every new profile.
const BORING_PATHS: &[&str] = &[
    "META-INF/MANIFEST.MF",
    "pack.mcmeta",
    "LICENSE",
    "LICENSE.txt",
    "README.md",
];

#[derive(Debug, Clone, Default)]
pub struct DetectOptions {
    /// Off for games where each mod lives in its own namespace (Factorio), so
    /// identical internal paths are normal rather than a conflict.
    pub check_file_overlap: bool,
    /// Used to name the winner of a file overlap. Empty when the game has no
    /// load order file, or the file was not found.
    pub load_order: LoadOrder,
    /// Metadata filenames every mod of this game carries, which therefore
    /// overlap everywhere and mean nothing.
    pub metadata_names: BTreeSet<String>,
}

/// What detection found, including the requirements it could not verify.
#[derive(Debug, Default)]
pub struct Detection {
    pub conflicts: Vec<Conflict>,
    /// Version requirements the tool could not read, so made no claim about —
    /// a Forge Maven range on a game marked semver, an unparseable version.
    /// Surfaced so the gap is visible rather than silently passing.
    pub unverified: usize,
}

pub fn detect(mods: &[ModEntry], opts: &DetectOptions) -> Detection {
    let mut conflicts = Vec::new();

    if opts.check_file_overlap {
        conflicts.extend(file_overlaps(mods, opts));
    }
    conflicts.extend(duplicate_ids(mods));
    let (deps, unverified) = dependency_problems(mods);
    conflicts.extend(deps);

    // Critical first, then stable by title so output does not jitter between runs.
    conflicts.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| a.title().cmp(&b.title()))
    });
    Detection {
        conflicts,
        unverified,
    }
}

fn file_overlaps(mods: &[ModEntry], opts: &DetectOptions) -> Vec<Conflict> {
    let mut by_path: HashMap<&str, Vec<String>> = HashMap::new();
    for m in mods {
        for f in &m.files {
            if is_boring(f, &opts.metadata_names) {
                continue;
            }
            by_path.entry(f.as_str()).or_default().push(m.id.clone());
        }
    }

    let mut out: Vec<Conflict> = by_path
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(path, mut owners)| {
            owners.sort();
            let winner = opts.load_order.winner(&owners).cloned();
            Conflict::FileOverlap {
                path: path.to_string(),
                mods: owners,
                winner,
                // Filled in later, by hashing, and only for paths that clashed.
                identical: false,
            }
        })
        .collect();
    out.sort_by_key(|c| c.title());
    out
}

pub fn is_boring(path: &str, metadata_names: &BTreeSet<String>) -> bool {
    let basename = path.rsplit('/').next().unwrap_or(path);
    if metadata_names.contains(basename) {
        return true;
    }
    BORING_PATHS
        .iter()
        .any(|b| path == *b || path.ends_with(&format!("/{b}")))
}

fn duplicate_ids(mods: &[ModEntry]) -> Vec<Conflict> {
    let mut by_symbol: HashMap<&Symbol, Vec<String>> = HashMap::new();
    for m in mods {
        for s in &m.provides {
            by_symbol.entry(s).or_default().push(m.id.clone());
        }
    }

    let mut out: Vec<Conflict> = by_symbol
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(symbol, mut owners)| {
            owners.sort();
            Conflict::DuplicateId {
                symbol: symbol.clone(),
                mods: owners,
            }
        })
        .collect();
    out.sort_by_key(|c| c.title());
    out
}

fn dependency_problems(mods: &[ModEntry]) -> (Vec<Conflict>, usize) {
    let installed: HashMap<&str, Option<&str>> = mods
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_deref()))
        .collect();

    let mut out = Vec::new();
    let mut unverified = 0usize;
    for m in mods {
        for dep in &m.requires {
            match dep.kind {
                DepKind::Incompatible => {
                    if installed.contains_key(dep.name.as_str()) {
                        out.push(Conflict::Incompatible {
                            mod_id: m.id.clone(),
                            other: dep.name.clone(),
                        });
                    }
                }
                DepKind::Required | DepKind::Optional => {
                    let Some(found) = installed.get(dep.name.as_str()) else {
                        // An absent optional dependency is not a problem.
                        if dep.kind == DepKind::Required {
                            out.push(Conflict::MissingDep {
                                mod_id: m.id.clone(),
                                dep: dep.clone(),
                            });
                        }
                        continue;
                    };
                    match check_version(dep, *found) {
                        Outcome::Satisfied => {}
                        Outcome::Violated => out.push(Conflict::VersionMismatch {
                            mod_id: m.id.clone(),
                            dep: dep.clone(),
                            found: found.unwrap_or_default().to_string(),
                        }),
                        // The presence dependency is met; only the *version*
                        // could not be checked. Never a false alarm — counted,
                        // not reported as a conflict.
                        Outcome::Unverified => unverified += 1,
                    }
                }
            }
        }
    }
    (out, unverified)
}

/// A dependency with no version requirement is satisfied by presence alone.
fn check_version(dep: &Dep, found: Option<&str>) -> Outcome {
    let (Some(req), Some(found)) = (dep.req.as_deref(), found) else {
        return Outcome::Satisfied;
    };
    versionreq::check(dep.syntax, req, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SymbolKind;

    fn opts() -> DetectOptions {
        DetectOptions {
            check_file_overlap: true,
            load_order: LoadOrder::default(),
            metadata_names: Default::default(),
        }
    }

    fn opts_with_order(order: &[&str]) -> DetectOptions {
        DetectOptions {
            check_file_overlap: true,
            load_order: LoadOrder {
                order: order.iter().map(|s| s.to_string()).collect(),
                disabled: Default::default(),
            },
            metadata_names: Default::default(),
        }
    }

    fn entry(id: &str, version: &str) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            version: Some(version.to_string()),
            files: Vec::new(),
            provides: vec![Symbol {
                kind: SymbolKind::ModId,
                name: id.to_string(),
            }],
            requires: Vec::new(),
            metadata_found: true,
        }
    }

    fn dep(name: &str, req: Option<&str>, kind: DepKind) -> Dep {
        Dep {
            name: name.to_string(),
            req: req.map(str::to_string),
            kind,
            syntax: Default::default(),
        }
    }

    /// The tests here predate the unverified count and care only about the
    /// conflicts, so this keeps them reading `detect(...)` as a list.
    fn detect(mods: &[ModEntry], opts: &DetectOptions) -> Vec<Conflict> {
        super::detect(mods, opts).conflicts
    }

    #[test]
    fn no_conflicts_in_a_clean_folder() {
        let mods = vec![entry("alpha", "1.0.0"), entry("beta", "1.0.0")];
        assert!(detect(&mods, &opts()).is_empty());
    }

    #[test]
    fn reports_two_mods_shipping_the_same_path() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        a.files = vec!["assets/stone.png".into()];
        b.files = vec!["assets/stone.png".into()];

        let found = detect(&[a, b], &opts());

        assert_eq!(
            found,
            vec![Conflict::FileOverlap {
                path: "assets/stone.png".into(),
                mods: vec!["alpha".into(), "beta".into()],
                winner: None,
                identical: false,
            }]
        );
    }

    #[test]
    fn the_load_order_names_the_winner_of_an_overlap() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        a.files = vec!["assets/stone.png".into()];
        b.files = vec!["assets/stone.png".into()];

        let found = detect(&[a, b], &opts_with_order(&["beta", "alpha"]));

        assert_eq!(
            found,
            vec![Conflict::FileOverlap {
                path: "assets/stone.png".into(),
                mods: vec!["alpha".into(), "beta".into()],
                winner: Some("alpha".into()),
                identical: false,
            }]
        );
        assert!(found[0].title().contains("alpha wins"));
    }

    #[test]
    fn ignores_overlap_on_boilerplate_paths() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        a.files = vec!["META-INF/MANIFEST.MF".into(), "alpha/pack.mcmeta".into()];
        b.files = vec!["META-INF/MANIFEST.MF".into(), "beta/pack.mcmeta".into()];

        assert!(detect(&[a, b], &opts()).is_empty());
    }

    #[test]
    fn the_games_own_metadata_filename_is_never_an_overlap() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        // Every Stardew mod ships a manifest.json; saying so every time is noise.
        a.files = vec!["manifest.json".into()];
        b.files = vec!["manifest.json".into()];

        let with_names = DetectOptions {
            metadata_names: ["manifest.json".to_string()].into_iter().collect(),
            ..opts()
        };

        assert!(detect(&[a.clone(), b.clone()], &with_names).is_empty());
        // Without the profile telling it so, it is just another shared path.
        assert_eq!(detect(&[a, b], &opts()).len(), 1);
    }

    #[test]
    fn skips_file_overlap_entirely_when_disabled() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        a.files = vec!["data.lua".into()];
        b.files = vec!["data.lua".into()];

        let disabled_overlap = DetectOptions {
            check_file_overlap: false,
            ..opts()
        };
        assert!(detect(&[a, b], &disabled_overlap).is_empty());
    }

    #[test]
    fn reports_two_mods_claiming_the_same_id() {
        let a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        b.provides = a.provides.clone();

        let found = detect(&[a, b], &opts());

        assert_eq!(
            found,
            vec![Conflict::DuplicateId {
                symbol: Symbol {
                    kind: SymbolKind::ModId,
                    name: "alpha".into()
                },
                mods: vec!["alpha".into(), "beta".into()],
            }]
        );
    }

    #[test]
    fn reports_a_required_dependency_that_is_not_installed() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("missing", None, DepKind::Required)];

        let found = detect(&[a], &opts());

        assert_eq!(found.len(), 1);
        assert!(matches!(found[0], Conflict::MissingDep { .. }));
    }

    #[test]
    fn stays_quiet_about_an_absent_optional_dependency() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("absent", None, DepKind::Optional)];

        assert!(detect(&[a], &opts()).is_empty());
    }

    #[test]
    fn reports_an_installed_dependency_with_the_wrong_version() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("beta", Some(">=2.0.0"), DepKind::Required)];
        let b = entry("beta", "1.5.0");

        let found = detect(&[a, b], &opts());

        assert_eq!(
            found,
            vec![Conflict::VersionMismatch {
                mod_id: "alpha".into(),
                dep: dep("beta", Some(">=2.0.0"), DepKind::Required),
                found: "1.5.0".into(),
            }]
        );
    }

    #[test]
    fn accepts_a_satisfied_version_requirement() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("beta", Some(">=1.0.0"), DepKind::Required)];
        let b = entry("beta", "1.5.0");

        assert!(detect(&[a, b], &opts()).is_empty());
    }

    #[test]
    fn checks_the_version_of_an_optional_dependency_that_is_present() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("beta", Some(">=2.0.0"), DepKind::Optional)];
        let b = entry("beta", "1.0.0");

        let found = detect(&[a, b], &opts());

        assert_eq!(found.len(), 1);
        assert!(matches!(found[0], Conflict::VersionMismatch { .. }));
    }

    #[test]
    fn reports_a_declared_incompatibility_when_the_other_mod_is_present() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("beta", None, DepKind::Incompatible)];
        let b = entry("beta", "1.0.0");

        let found = detect(&[a, b], &opts());

        assert_eq!(
            found,
            vec![Conflict::Incompatible {
                mod_id: "alpha".into(),
                other: "beta".into(),
            }]
        );
    }


    #[test]
    fn an_unparseable_version_never_raises_a_false_alarm() {
        let mut a = entry("alpha", "1.0.0");
        a.requires = vec![dep("beta", Some(">=1.0.0"), DepKind::Required)];
        let b = entry("beta", "not-a-version");

        assert!(detect(&[a, b], &opts()).is_empty());
    }

    #[test]
    fn critical_conflicts_sort_before_warnings() {
        let mut a = entry("alpha", "1.0.0");
        let mut b = entry("beta", "1.0.0");
        a.files = vec!["assets/stone.png".into()];
        b.files = vec!["assets/stone.png".into()];
        a.requires = vec![dep("missing", None, DepKind::Required)];

        let found = detect(&[a, b], &opts());

        assert_eq!(found.len(), 2);
        assert!(matches!(found[0], Conflict::MissingDep { .. }));
        assert!(matches!(found[1], Conflict::FileOverlap { .. }));
    }
}

