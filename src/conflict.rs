//! Conflict detection. Pure function of `Vec<ModEntry>` — no I/O, no game
//! knowledge, fully testable without touching the filesystem.

use std::collections::HashMap;

use crate::loadorder::LoadOrder;
use crate::model::{Conflict, Dep, DepKind, ModEntry, Symbol};

/// Paths that collide in practically every mod and never mean anything.
const BORING_PATHS: &[&str] = &[
    "META-INF/MANIFEST.MF",
    "META-INF/mods.toml",
    "pack.mcmeta",
    "fabric.mod.json",
    "info.json",
    "modDesc.xml",
    "LICENSE",
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
}

pub fn detect(mods: &[ModEntry], opts: &DetectOptions) -> Vec<Conflict> {
    let mut conflicts = Vec::new();

    if opts.check_file_overlap {
        conflicts.extend(file_overlaps(mods, &opts.load_order));
    }
    conflicts.extend(duplicate_ids(mods));
    conflicts.extend(dependency_problems(mods));

    // Critical first, then stable by title so output does not jitter between runs.
    conflicts.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then_with(|| a.title().cmp(&b.title()))
    });
    conflicts
}

fn file_overlaps(mods: &[ModEntry], load_order: &LoadOrder) -> Vec<Conflict> {
    let mut by_path: HashMap<&str, Vec<String>> = HashMap::new();
    for m in mods {
        for f in &m.files {
            if is_boring(f) {
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
            let winner = load_order.winner(&owners).cloned();
            Conflict::FileOverlap {
                path: path.to_string(),
                mods: owners,
                winner,
            }
        })
        .collect();
    out.sort_by_key(|c| c.title());
    out
}

fn is_boring(path: &str) -> bool {
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

fn dependency_problems(mods: &[ModEntry]) -> Vec<Conflict> {
    let installed: HashMap<&str, Option<&str>> = mods
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_deref()))
        .collect();

    let mut out = Vec::new();
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
                    if let Some(bad) = version_mismatch(dep, *found) {
                        out.push(Conflict::VersionMismatch {
                            mod_id: m.id.clone(),
                            dep: dep.clone(),
                            found: bad,
                        });
                    }
                }
            }
        }
    }
    out
}

/// `Some(found_version)` when the installed version violates the requirement.
/// Unparseable versions or requirements are treated as satisfied — guessing
/// would produce false positives, and a false alarm is worse than a miss here.
fn version_mismatch(dep: &Dep, found: Option<&str>) -> Option<String> {
    let req_str = dep.req.as_deref()?;
    let found_str = found?;

    let req = semver::VersionReq::parse(req_str).ok()?;
    let version = parse_version(found_str)?;

    if req.matches(&version) {
        None
    } else {
        Some(found_str.to_string())
    }
}

/// Games are loose about version arity: Factorio ships `0.18` and `1.1.0`
/// alike. Pad to three components so semver can read both.
fn parse_version(raw: &str) -> Option<semver::Version> {
    let padded = match raw.matches('.').count() {
        0 => format!("{raw}.0.0"),
        1 => format!("{raw}.0"),
        _ => raw.to_string(),
    };
    semver::Version::parse(&padded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SymbolKind;

    fn opts() -> DetectOptions {
        DetectOptions {
            check_file_overlap: true,
            load_order: LoadOrder::default(),
        }
    }

    fn opts_with_order(order: &[&str]) -> DetectOptions {
        DetectOptions {
            check_file_overlap: true,
            load_order: LoadOrder {
                order: order.iter().map(|s| s.to_string()).collect(),
                disabled: Default::default(),
            },
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
        }
    }

    fn dep(name: &str, req: Option<&str>, kind: DepKind) -> Dep {
        Dep {
            name: name.to_string(),
            req: req.map(str::to_string),
            kind,
        }
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
    fn short_versions_compare_correctly() {
        assert_eq!(parse_version("0.18"), Some(semver::Version::new(0, 18, 0)));
        assert_eq!(parse_version("2"), Some(semver::Version::new(2, 0, 0)));
        assert_eq!(parse_version("1.2.3"), Some(semver::Version::new(1, 2, 3)));
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

