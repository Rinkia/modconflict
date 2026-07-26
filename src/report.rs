//! Output formats: a human summary, and `--json` for scripts and CI.

use serde::Serialize;

use crate::loadorder::LoadOrder;
use crate::model::{Conflict, Severity};
use crate::profile::Profile;

pub struct Report<'a> {
    pub profile: &'a Profile,
    pub mods_scanned: usize,
    pub mods_disabled: usize,
    pub load_order_known: bool,
    /// Binary archives whose contents were expanded into the scan, by format.
    pub containers: Vec<String>,
    /// Binary plugins parsed at the record level.
    pub plugins_read: usize,
    /// Mods whose metadata the profile actually understood. Well short of
    /// `mods_scanned` means the profile is wrong for this folder, not that the
    /// folder is clean.
    pub mods_with_metadata: usize,
    /// Version requirements the tool could not read a claim from. Reported so
    /// the gap is visible rather than silently passing.
    pub unverified_requirements: usize,
    /// Everything that could not be read. Reported rather than printed to
    /// stderr, so a `--json` consumer sees it too.
    pub warnings: &'a [String],
    /// Name and profile of the mod manager that supplied the ordering.
    pub manager: Option<(&'a str, &'a str)>,
    pub conflicts: &'a [Conflict],
}

impl Report<'_> {
    pub fn critical_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.severity() == Severity::Critical)
            .count()
    }

    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Whether anything found is worth doing something about.
    ///
    /// `Info` findings — identical copies, duplicate installs — are reported
    /// but change nothing in the game, so they must not fail a pre-launch
    /// check. Exiting non-zero over them would train people to ignore the
    /// exit code.
    pub fn has_actionable(&self) -> bool {
        self.conflicts.iter().any(|c| c.severity() > Severity::Info)
    }

    /// How much of the folder the profile actually understood, 0.0 to 1.0.
    /// The health signal: well below 1.0 means the profile is wrong for this
    /// folder, not that the folder is clean.
    pub fn metadata_coverage(&self) -> f64 {
        if self.mods_scanned == 0 {
            return 1.0;
        }
        self.mods_with_metadata as f64 / self.mods_scanned as f64
    }
}

pub fn print_text(report: &Report) {
    print!("{}", text(report));
}

pub fn print_json(report: &Report) -> anyhow::Result<()> {
    println!("{}", json(report)?);
    Ok(())
}

/// The human report, as a string so it can be snapshotted rather than only
/// eyeballed.
pub fn text(report: &Report) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}: scanned {} {}{}",
        report.profile.display_name,
        report.mods_scanned,
        if report.mods_scanned == 1 {
            "mod"
        } else {
            "mods"
        },
        match report.mods_disabled {
            0 => String::new(),
            n => format!(" ({n} disabled, skipped)"),
        }
    );

    if !report.containers.is_empty() {
        let _ = writeln!(
            out,
            "read {} binary {} ({})",
            report.containers.len(),
            if report.containers.len() == 1 {
                "archive"
            } else {
                "archives"
            },
            summarize(&report.containers)
        );
    }

    if report.plugins_read > 0 {
        let _ = writeln!(out, "read {} plugins at record level", report.plugins_read);
    }

    // Silence when every mod was understood; the miss is the news.
    if report.mods_with_metadata < report.mods_scanned {
        let _ = writeln!(
            out,
            "warning: read metadata for only {} of {} mods ({:.0}%) — the rest fall back to \
             their filename. If that is most of them, this is the wrong game profile.",
            report.mods_with_metadata,
            report.mods_scanned,
            report.metadata_coverage() * 100.0
        );
    }

    for warning in report.warnings {
        let _ = writeln!(out, "warning: {warning}");
    }

    if report.unverified_requirements > 0 {
        let _ = writeln!(
            out,
            "note: {} version {} could not be checked (unreadable dialect) — \
             treated as satisfied",
            report.unverified_requirements,
            if report.unverified_requirements == 1 {
                "requirement"
            } else {
                "requirements"
            }
        );
    }

    match report.manager {
        Some((name, profile)) => {
            let _ = writeln!(out, "load order from {name}, profile \"{profile}\"");
        }
        // Only worth saying for a game that has a load order to find.
        None if !report.load_order_known && report.profile.load_order.is_some() => {
            let _ = writeln!(
                out,
                "note: no load order file found — overlap winners are unknown"
            );
        }
        None => {}
    }

    if report.is_clean() {
        let _ = writeln!(out, "no conflicts found");
        return out;
    }

    let _ = writeln!(
        out,
        "{} {} ({} critical)\n",
        report.conflicts.len(),
        if report.conflicts.len() == 1 {
            "conflict"
        } else {
            "conflicts"
        },
        report.critical_count()
    );
    for c in report.conflicts {
        let _ = writeln!(out, "[{}] {}", c.severity(), c.title());
    }
    let _ = writeln!(out, "\nRun with --tui for details.");
    out
}

/// The JSON envelope. Field names are part of the tool's contract, so they are
/// spelled out here rather than derived from internal type names.
#[derive(Serialize)]
struct JsonReport<'a> {
    game: &'a str,
    game_display_name: &'a str,
    mods_scanned: usize,
    mods_disabled: usize,
    load_order_known: bool,
    containers_read: usize,
    plugins_read: usize,
    mods_with_metadata: usize,
    unverified_requirements: usize,
    conflict_count: usize,
    critical_count: usize,
    warnings: &'a [String],
    manager: Option<&'a str>,
    manager_profile: Option<&'a str>,
    conflicts: Vec<JsonConflict<'a>>,
}

#[derive(Serialize)]
struct JsonConflict<'a> {
    severity: Severity,
    title: String,
    detail: String,
    #[serde(flatten)]
    data: &'a Conflict,
}

pub fn json(report: &Report) -> anyhow::Result<String> {
    let json = JsonReport {
        game: &report.profile.name,
        game_display_name: &report.profile.display_name,
        mods_scanned: report.mods_scanned,
        mods_disabled: report.mods_disabled,
        load_order_known: report.load_order_known,
        containers_read: report.containers.len(),
        plugins_read: report.plugins_read,
        mods_with_metadata: report.mods_with_metadata,
        unverified_requirements: report.unverified_requirements,
        conflict_count: report.conflicts.len(),
        critical_count: report.critical_count(),
        warnings: report.warnings,
        manager: report.manager.map(|(name, _)| name),
        manager_profile: report.manager.map(|(_, profile)| profile),
        conflicts: report
            .conflicts
            .iter()
            .map(|c| JsonConflict {
                severity: c.severity(),
                title: c.title(),
                detail: c.detail(),
                data: c,
            })
            .collect(),
    };

    Ok(serde_json::to_string_pretty(&json)?)
}

/// `["a", "a", "b"]` -> `"a x2, b"`, so the note stays one line however many
/// archives a mod folder holds.
fn summarize(items: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for item in items {
        *counts.entry(item.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(name, n)| {
            if n == 1 {
                name.to_string()
            } else {
                format!("{name} x{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many mods the load order switched off.
pub fn disabled_count(load_order: &LoadOrder, ids: &[String]) -> usize {
    ids.iter().filter(|id| load_order.is_disabled(id)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dep, DepKind};
    use crate::profile::{by_name, load_all};

    fn profile() -> Profile {
        by_name(&load_all(None).unwrap(), "factorio")
            .unwrap()
            .clone()
    }

    fn conflicts() -> Vec<Conflict> {
        vec![
            Conflict::MissingDep {
                mod_id: "alpha".into(),
                dep: Dep {
                    name: "base".into(),
                    req: Some(">=2.0.0".into()),
                    kind: DepKind::Required,
                    syntax: Default::default(),
                },
            },
            Conflict::FileOverlap {
                path: "assets/stone.png".into(),
                mods: vec!["beta".into(), "gamma".into()],
                winner: Some("gamma".into()),
                identical: false,
            },
        ]
    }

    fn json_of(report: &Report) -> serde_json::Value {
        let conflicts: Vec<_> = report
            .conflicts
            .iter()
            .map(|c| {
                serde_json::to_value(JsonConflict {
                    severity: c.severity(),
                    title: c.title(),
                    detail: c.detail(),
                    data: c,
                })
                .unwrap()
            })
            .collect();
        serde_json::Value::Array(conflicts)
    }

    #[test]
    fn json_tags_each_conflict_with_its_kind_and_severity() {
        let profile = profile();
        let conflicts = conflicts();
        let report = Report {
            profile: &profile,
            mods_scanned: 3,
            mods_disabled: 0,
            load_order_known: true,
            containers: Vec::new(),
            plugins_read: 0,
            mods_with_metadata: 3,
            unverified_requirements: 0,
            warnings: &[],
            manager: None,
            conflicts: &conflicts,
        };

        let json = json_of(&report);

        assert_eq!(json[0]["kind"], "missing_dep");
        assert_eq!(json[0]["severity"], "critical");
        assert_eq!(json[0]["mod_id"], "alpha");
        assert_eq!(json[0]["dep"]["name"], "base");
        assert_eq!(json[0]["dep"]["kind"], "required");

        assert_eq!(json[1]["kind"], "file_overlap");
        assert_eq!(json[1]["severity"], "warning");
        assert_eq!(json[1]["winner"], "gamma");
        assert_eq!(json[1]["mods"][0], "beta");
    }

    #[test]
    fn json_carries_the_human_title_and_detail_too() {
        let profile = profile();
        let conflicts = conflicts();
        let report = Report {
            profile: &profile,
            mods_scanned: 3,
            mods_disabled: 0,
            load_order_known: true,
            containers: Vec::new(),
            plugins_read: 0,
            mods_with_metadata: 3,
            unverified_requirements: 0,
            warnings: &[],
            manager: None,
            conflicts: &conflicts,
        };

        let json = json_of(&report);

        assert!(json[0]["title"].as_str().unwrap().contains("alpha"));
        assert!(json[1]["detail"].as_str().unwrap().contains("gamma"));
    }

    #[test]
    fn counts_criticals_separately_from_warnings() {
        let profile = profile();
        let conflicts = conflicts();
        let report = Report {
            profile: &profile,
            mods_scanned: 3,
            mods_disabled: 1,
            load_order_known: true,
            containers: Vec::new(),
            plugins_read: 0,
            mods_with_metadata: 3,
            unverified_requirements: 0,
            warnings: &[],
            manager: None,
            conflicts: &conflicts,
        };

        assert_eq!(report.conflicts.len(), 2);
        assert_eq!(report.critical_count(), 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn counts_disabled_mods() {
        let mut order = LoadOrder::default();
        order.disabled.insert("beta".to_string());

        let count = disabled_count(&order, &["alpha".to_string(), "beta".to_string()]);

        assert_eq!(count, 1);
    }
}
