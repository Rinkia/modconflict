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
}

pub fn print_text(report: &Report) {
    println!(
        "{}: scanned {} mods{}",
        report.profile.display_name,
        report.mods_scanned,
        match report.mods_disabled {
            0 => String::new(),
            n => format!(" ({n} disabled, skipped)"),
        }
    );

    if !report.load_order_known && report.profile.load_order.is_some() {
        println!("note: no load order file found — overlap winners are unknown");
    }

    if report.is_clean() {
        println!("no conflicts found");
        return;
    }

    println!(
        "{} conflicts ({} critical)\n",
        report.conflicts.len(),
        report.critical_count()
    );
    for c in report.conflicts {
        println!("[{}] {}", c.severity(), c.title());
    }
    println!("\nRun with --tui for details.");
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
    conflict_count: usize,
    critical_count: usize,
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

pub fn print_json(report: &Report) -> anyhow::Result<()> {
    let json = JsonReport {
        game: &report.profile.name,
        game_display_name: &report.profile.display_name,
        mods_scanned: report.mods_scanned,
        mods_disabled: report.mods_disabled,
        load_order_known: report.load_order_known,
        conflict_count: report.conflicts.len(),
        critical_count: report.critical_count(),
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

    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
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
        by_name(&load_all(None).unwrap(), "factorio").unwrap().clone()
    }

    fn conflicts() -> Vec<Conflict> {
        vec![
            Conflict::MissingDep {
                mod_id: "alpha".into(),
                dep: Dep {
                    name: "base".into(),
                    req: Some(">=2.0.0".into()),
                    kind: DepKind::Required,
                },
            },
            Conflict::FileOverlap {
                path: "assets/stone.png".into(),
                mods: vec!["beta".into(), "gamma".into()],
                winner: Some("gamma".into()),
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
