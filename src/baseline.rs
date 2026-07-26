//! Baseline: accept today's conflicts, report only what is new.
//!
//! A person with 300 mods already installed runs the tool and bounces off a
//! wall of pre-existing findings. A baseline is how they *start*: record the
//! current state as accepted, and from then on the report shows only conflicts
//! that appeared since.
//!
//! The baseline file is a JSON report the tool itself produced —
//! `modconflict <folder> --json > baseline.json`. There is no separate format
//! to learn, and no second way for the two sides to drift: both the current
//! conflicts and the baselined ones are the same `Conflict` type, matched on
//! `Conflict::identity`.
//!
//! Baselined findings are counted in the report. Hiding them silently would be
//! the same lie by omission the ignore config already refuses.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::Conflict;

/// The identities of the conflicts a baseline accepts.
#[derive(Debug, Default, Clone)]
pub struct Baseline(HashSet<String>);

/// Just enough of the JSON report to pull the conflicts back out. Every other
/// field is ignored, so an older or newer report still loads.
#[derive(Deserialize)]
struct RawReport {
    #[serde(default)]
    conflicts: Vec<Conflict>,
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let report: RawReport = serde_json::from_str(&text).with_context(|| {
            format!(
                "parsing {} — expected a report from `modconflict --json`",
                path.display()
            )
        })?;
        Ok(Baseline(
            report.conflicts.iter().map(Conflict::identity).collect(),
        ))
    }

    /// Drop the conflicts the baseline already accepted, returning how many were
    /// removed so the report can state the count.
    pub fn apply(&self, conflicts: &mut Vec<Conflict>) -> usize {
        if self.0.is_empty() {
            return 0;
        }
        let before = conflicts.len();
        conflicts.retain(|c| !self.0.contains(&c.identity()));
        before - conflicts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dep, DepKind};

    fn overlap(path: &str, mods: &[&str]) -> Conflict {
        Conflict::FileOverlap {
            path: path.into(),
            mods: mods.iter().map(|s| s.to_string()).collect(),
            winner: None,
            identical: false,
        }
    }

    fn missing(mod_id: &str, dep: &str) -> Conflict {
        Conflict::MissingDep {
            mod_id: mod_id.into(),
            dep: Dep {
                name: dep.into(),
                req: None,
                kind: DepKind::Required,
                syntax: Default::default(),
            },
        }
    }

    fn baseline(conflicts: &[Conflict]) -> Baseline {
        Baseline(conflicts.iter().map(Conflict::identity).collect())
    }

    #[test]
    fn known_conflicts_are_hidden_new_ones_survive() {
        let base = baseline(&[overlap("a.png", &["x", "y"]), missing("x", "dep")]);
        let mut current = vec![
            overlap("a.png", &["x", "y"]), // in baseline
            overlap("b.png", &["x", "z"]), // new
            missing("x", "dep"),           // in baseline
        ];

        let hidden = base.apply(&mut current);

        assert_eq!(hidden, 2);
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].path(), Some("b.png"));
    }

    #[test]
    fn identity_ignores_a_changed_winner() {
        // The same overlap gains a winner between runs; still the same finding.
        let base = baseline(&[overlap("a.png", &["x", "y"])]);
        let mut current = vec![Conflict::FileOverlap {
            path: "a.png".into(),
            mods: vec!["x".into(), "y".into()],
            winner: Some("y".into()),
            identical: true,
        }];

        assert_eq!(base.apply(&mut current), 1);
        assert!(current.is_empty());
    }

    #[test]
    fn mod_order_does_not_change_identity() {
        let base = baseline(&[overlap("a.png", &["y", "x"])]);
        let mut current = vec![overlap("a.png", &["x", "y"])];
        assert_eq!(base.apply(&mut current), 1);
    }

    #[test]
    fn an_empty_baseline_hides_nothing() {
        let base = Baseline::default();
        let mut current = vec![overlap("a.png", &["x", "y"])];
        assert_eq!(base.apply(&mut current), 0);
        assert_eq!(current.len(), 1);
    }

    #[test]
    fn loads_from_a_json_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baseline.json");
        // A minimal report shape: only the conflicts array is read.
        std::fs::write(
            &path,
            r#"{"conflicts":[{"kind":"file_overlap","path":"a.png",
               "mods":["x","y"],"winner":null,"identical":false}]}"#,
        )
        .unwrap();

        let base = Baseline::load(&path).unwrap();
        let mut current = vec![overlap("a.png", &["x", "y"]), overlap("b.png", &["x", "y"])];

        assert_eq!(base.apply(&mut current), 1);
        assert_eq!(current[0].path(), Some("b.png"));
    }
}
