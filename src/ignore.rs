//! User ignore rules.
//!
//! A folder full of deliberate compatibility patches produces the same warning
//! forever, and a person who sees 30 warnings they meant to accept stops
//! reading warnings at all — the exact failure the hashing pass was built to
//! avoid, walking back in through another door. So the user can say "I know,
//! stop": a `modconflict.toml` in the mod folder (or `--config <FILE>`) lists
//! findings to suppress.
//!
//! Every suppressed finding is *counted* in the report. The tool never goes
//! silent — the same rule already applied to unverified requirements. A config
//! that hides four conflicts makes the report say so.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;

use crate::manager::Manager;
use crate::model::{Conflict, ConflictKind, Severity};

/// The file looked for in the mod folder when `--config` is not given.
pub const CONFIG_NAME: &str = "modconflict.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    ignore: Vec<RawRule>,
    #[serde(default)]
    settings: Settings,
}

/// Persistent flag defaults, so a large install stops retyping `--manager mo2
/// --profiles … --no-records` every run. A flag on the command line always
/// wins over the file. TOML keys mirror the CLI (`no-records`, `fail-on`).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Settings {
    pub game: Option<String>,
    pub profiles: Option<PathBuf>,
    pub manager: Option<Manager>,
    pub no_records: Option<bool>,
    pub no_hash: Option<bool>,
    pub fail_on: Option<Severity>,
}

/// A loaded config file: the flag defaults and the ignore rules. Both come from
/// the one `modconflict.toml`, parsed once by the CLI layer.
pub struct Config {
    pub settings: Settings,
    pub rules: Rules,
}

impl Config {
    /// Load `override_path` if given, else `modconflict.toml` in `mod_dir`. An
    /// explicit `--config` that does not exist is a user error; an absent
    /// default file is simply "no config".
    pub fn load(mod_dir: &Path, override_path: Option<&Path>) -> Result<Self> {
        let (path, required) = match override_path {
            Some(p) => (p.to_path_buf(), true),
            None => (mod_dir.join(CONFIG_NAME), false),
        };
        if !path.exists() {
            if required {
                bail!("config file not found: {}", path.display());
            }
            return Ok(Config {
                settings: Settings::default(),
                rules: Rules::default(),
            });
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let raw: RawConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let rules = raw
            .ignore
            .into_iter()
            .map(Rule::compile)
            .collect::<Result<Vec<_>>>()?;
        Ok(Config {
            settings: raw.settings,
            rules: Rules(rules),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    /// Glob against the conflict's path (file overlaps only), e.g. `textures/**`.
    #[serde(default)]
    path: Option<String>,
    /// Mods that must all be involved in the conflict for the rule to apply —
    /// the way to silence one deliberate pair without silencing the path
    /// everywhere.
    #[serde(default)]
    mods: Vec<String>,
    /// Restrict the rule to one kind of conflict.
    #[serde(default)]
    kind: Option<ConflictKind>,
    /// Free text for whoever edits the file; the tool never reads it, but
    /// rejecting it would make the format hostile to document.
    #[serde(default, rename = "reason")]
    _reason: Option<String>,
}

/// One compiled ignore rule. A conflict is suppressed when it matches every
/// field the rule specifies (an unset field matches anything).
#[derive(Debug, Clone)]
struct Rule {
    path: Option<GlobMatcher>,
    /// Lower-cased, so `Alpha` in the config matches an `alpha` mod id — case
    /// folding, like everywhere else in the tool.
    mods: Vec<String>,
    kind: Option<ConflictKind>,
}

impl Rule {
    fn compile(raw: RawRule) -> Result<Self> {
        // An all-empty rule matches every conflict — it would silence the whole
        // report, which is exactly the "lie by omission" this tool refuses.
        if raw.path.is_none() && raw.mods.is_empty() && raw.kind.is_none() {
            bail!("an ignore rule must set at least one of `path`, `mods`, or `kind`");
        }
        let path = raw
            .path
            .map(|p| {
                // Case-insensitive: mod paths are stored in the casing first
                // seen, but the game folds case, so a rule should too.
                GlobBuilder::new(&p)
                    .case_insensitive(true)
                    .build()
                    .map(|g| g.compile_matcher())
                    .with_context(|| format!("invalid path glob {p:?}"))
            })
            .transpose()?;
        Ok(Rule {
            path,
            mods: raw.mods.iter().map(|m| m.to_ascii_lowercase()).collect(),
            kind: raw.kind,
        })
    }

    fn matches(&self, c: &Conflict) -> bool {
        if let Some(kind) = self.kind {
            if c.kind() != kind {
                return false;
            }
        }
        if let Some(glob) = &self.path {
            match c.path() {
                Some(p) if glob.is_match(p) => {}
                _ => return false,
            }
        }
        if !self.mods.is_empty() {
            let involved: Vec<String> = c.mods().iter().map(|m| m.to_ascii_lowercase()).collect();
            if !self
                .mods
                .iter()
                .all(|want| involved.iter().any(|have| have == want))
            {
                return false;
            }
        }
        true
    }
}

/// The loaded ignore rules. Empty when no config exists.
#[derive(Debug, Default, Clone)]
pub struct Rules(Vec<Rule>);

impl Rules {
    /// Remove the suppressed conflicts, returning how many were dropped so the
    /// report can state the count.
    pub fn suppress(&self, conflicts: &mut Vec<Conflict>) -> usize {
        if self.0.is_empty() {
            return 0;
        }
        let before = conflicts.len();
        conflicts.retain(|c| !self.0.iter().any(|r| r.matches(c)));
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

    fn rules(toml: &str) -> Rules {
        let raw: RawConfig = toml::from_str(toml).unwrap();
        Rules(
            raw.ignore
                .into_iter()
                .map(Rule::compile)
                .map(Result::unwrap)
                .collect(),
        )
    }

    #[test]
    fn a_path_glob_suppresses_matching_overlaps_only() {
        let rules = rules(
            r#"[[ignore]]
path = "textures/**"
"#,
        );
        let mut conflicts = vec![
            overlap("textures/armor/iron.dds", &["a", "b"]),
            overlap("meshes/sword.nif", &["a", "b"]),
        ];

        let n = rules.suppress(&mut conflicts);

        assert_eq!(n, 1);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path(), Some("meshes/sword.nif"));
    }

    #[test]
    fn a_path_glob_is_case_insensitive() {
        let rules = rules(
            r#"[[ignore]]
path = "Textures/**"
"#,
        );
        let mut conflicts = vec![overlap("textures/iron.dds", &["a", "b"])];
        assert_eq!(rules.suppress(&mut conflicts), 1);
    }

    #[test]
    fn a_mods_rule_needs_every_named_mod_present() {
        let rules = rules(
            r#"[[ignore]]
mods = ["Alpha", "Beta"]
"#,
        );
        let mut conflicts = vec![
            overlap("a.png", &["alpha", "beta"]), // both present, case-folded
            overlap("b.png", &["alpha", "gamma"]), // beta missing
        ];

        let n = rules.suppress(&mut conflicts);

        assert_eq!(n, 1);
        assert_eq!(conflicts[0].path(), Some("b.png"));
    }

    #[test]
    fn kind_and_path_combine_as_and() {
        let rules = rules(
            r#"[[ignore]]
path = "**/*.png"
kind = "file_overlap"
"#,
        );
        let mut conflicts = vec![
            overlap("assets/a.png", &["x", "y"]),
            missing("x", "dep"), // right nothing-to-match: no path, wrong kind
        ];

        let n = rules.suppress(&mut conflicts);

        assert_eq!(n, 1);
        assert!(matches!(conflicts[0], Conflict::MissingDep { .. }));
    }

    #[test]
    fn a_kind_only_rule_suppresses_all_of_that_kind() {
        let rules = rules(
            r#"[[ignore]]
kind = "missing_dep"
"#,
        );
        let mut conflicts = vec![missing("x", "dep"), overlap("a.png", &["x", "y"])];
        assert_eq!(rules.suppress(&mut conflicts), 1);
        assert!(matches!(conflicts[0], Conflict::FileOverlap { .. }));
    }

    #[test]
    fn an_empty_rule_is_refused_rather_than_silencing_everything() {
        let raw: RawConfig = toml::from_str("[[ignore]]\nreason = \"oops\"\n").unwrap();
        assert!(Rule::compile(raw.ignore.into_iter().next().unwrap()).is_err());
    }

    #[test]
    fn no_rules_suppress_nothing() {
        let rules = Rules(Vec::new());
        let mut conflicts = vec![overlap("a.png", &["x", "y"])];
        assert_eq!(rules.suppress(&mut conflicts), 0);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn an_absent_default_config_is_no_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(dir.path(), None).unwrap();
        assert!(config.rules.0.is_empty());
        assert!(config.settings.game.is_none());
    }

    #[test]
    fn a_missing_explicit_config_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let nowhere = dir.path().join("nope.toml");
        assert!(Config::load(dir.path(), Some(&nowhere)).is_err());
    }

    #[test]
    fn settings_are_read_alongside_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(CONFIG_NAME),
            "[settings]\nmanager = \"mo2\"\nno-records = true\nfail-on = \"critical\"\n\n\
             [[ignore]]\nkind = \"file_overlap\"\n",
        )
        .unwrap();

        let config = Config::load(dir.path(), None).unwrap();

        assert_eq!(config.settings.manager, Some(Manager::Mo2));
        assert_eq!(config.settings.no_records, Some(true));
        assert_eq!(config.settings.fail_on, Some(Severity::Critical));
        assert_eq!(config.rules.0.len(), 1);
    }
}
