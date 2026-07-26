//! Does an installed version satisfy a requirement?
//!
//! The honest answer has three values, not two. `semver` reads `>=1.1.0`, but
//! Forge writes `[36,)` and Fabric writes `1.20.x`, and feeding those to
//! `semver` fails. The old code treated a failed parse as *satisfied* — a
//! deliberate choice, because a false alarm is worse than a miss, but a silent
//! one: nobody could tell a real pass from a requirement the tool simply could
//! not read.
//!
//! So a check returns `Satisfied`, `Violated`, or `Unverified`, and the last is
//! counted in the report instead of hidden. Games declare which dialect they
//! use in the profile; the default is semver.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The installed version meets the requirement.
    Satisfied,
    /// It does not — a real conflict.
    Violated,
    /// The requirement or the version could not be read, so no claim is made.
    /// A miss, never a false alarm.
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VersionSyntax {
    /// `>=1.1.0`, `^1.2`, and `1.20.x` / `1.20.*` glob patterns.
    #[default]
    Semver,
    /// Maven ranges: `[36,)`, `[1.19,1.20)`, `(,2.0]`, a bare `1.0` meaning a
    /// soft `>=`. Forge and NeoForge.
    Maven,
}

pub fn check(syntax: VersionSyntax, req: &str, found: &str) -> Outcome {
    let Some(version) = parse_version(found) else {
        return Outcome::Unverified;
    };
    match syntax {
        VersionSyntax::Semver => check_semver(req, &version),
        VersionSyntax::Maven => check_maven(req, &version),
    }
}

/// Games are loose about version arity: Factorio ships `0.18`, Forge ships `36`.
/// Pad to three components so `semver::Version` can read both.
fn parse_version(raw: &str) -> Option<semver::Version> {
    // A leading `v`, as Bannerlord writes, is not part of the number.
    let raw = raw.trim().trim_start_matches(['v', 'V']);
    // Anything past a `+build` or `-pre` tag is not needed for comparison and
    // often is not valid semver; keep only the numeric core.
    let core: String = raw
        .split(['+', '-'])
        .next()
        .unwrap_or(raw)
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if core.is_empty() {
        return None;
    }
    let padded = match core.matches('.').count() {
        0 => format!("{core}.0.0"),
        1 => format!("{core}.0"),
        2 => core,
        // More than three components (Farming Simulator's 1.0.0.0): keep three.
        _ => core.splitn(4, '.').take(3).collect::<Vec<_>>().join("."),
    };
    semver::Version::parse(&padded).ok()
}

fn check_semver(req: &str, version: &semver::Version) -> Outcome {
    let normalized = normalize_glob(req);
    let Ok(parsed) = semver::VersionReq::parse(&normalized) else {
        return Outcome::Unverified;
    };
    if parsed.matches(version) {
        Outcome::Satisfied
    } else {
        Outcome::Violated
    }
}

/// Rewrite `1.20.x` / `1.20.*` into a range `semver` understands.
///
/// `1.20.x` pins major and minor: `>=1.20.0, <1.21.0`. `1.x` pins only major:
/// `>=1.0.0, <2.0.0`. A pattern that does not fit either shape is left as-is
/// for `semver` to accept or reject.
fn normalize_glob(req: &str) -> String {
    let trimmed = req.trim();
    let lower = trimmed.to_ascii_lowercase();
    let Some(prefix) = lower
        .strip_suffix(".x")
        .or_else(|| lower.strip_suffix(".*"))
    else {
        return trimmed.to_string();
    };

    let parts: Vec<&str> = prefix.split('.').collect();
    match parts.as_slice() {
        [major] => match major.parse::<u64>() {
            Ok(m) => format!(">={m}.0.0, <{}.0.0", m + 1),
            Err(_) => trimmed.to_string(),
        },
        [major, minor] => match (major.parse::<u64>(), minor.parse::<u64>()) {
            (Ok(m), Ok(n)) => format!(">={m}.{n}.0, <{m}.{}.0", n + 1),
            _ => trimmed.to_string(),
        },
        _ => trimmed.to_string(),
    }
}

/// A single Maven bound: a version and whether it is inclusive.
struct Bound {
    version: semver::Version,
    inclusive: bool,
}

fn check_maven(req: &str, version: &semver::Version) -> Outcome {
    let req = req.trim();

    // A bare version with no bracket is a *soft* requirement — a recommendation,
    // not a constraint. Maven treats the build as free to pick anything, so
    // there is nothing to violate.
    if !req.starts_with(['[', '(']) {
        return match parse_version(req) {
            Some(_) => Outcome::Satisfied,
            None => Outcome::Unverified,
        };
    }

    // Maven allows several comma-joined ranges, which are OR-ed. Splitting on
    // the boundary between `]`/`)` and `[`/`(` keeps each range's inner comma
    // intact.
    let ranges = split_ranges(req);
    if ranges.is_empty() {
        return Outcome::Unverified;
    }

    let mut any_readable = false;
    for range in ranges {
        match matches_range(&range, version) {
            Some(true) => return Outcome::Satisfied,
            Some(false) => any_readable = true,
            None => {}
        }
    }
    // Readable and matched none: a real violation. Not readable at all:
    // unverified rather than a guess.
    if any_readable {
        Outcome::Violated
    } else {
        Outcome::Unverified
    }
}

/// Split `[1,2),[3,)` into `["[1,2)", "[3,)"]`.
fn split_ranges(req: &str) -> Vec<String> {
    let mut ranges = Vec::new();
    let mut current = String::new();
    let mut chars = req.chars().peekable();

    while let Some(c) = chars.next() {
        current.push(c);
        if (c == ']' || c == ')') && chars.peek() == Some(&',') {
            chars.next(); // consume the joining comma
            ranges.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        ranges.push(current);
    }
    ranges
}

/// `Some(true)`/`Some(false)` when the range is readable, `None` when it is not.
fn matches_range(range: &str, version: &semver::Version) -> Option<bool> {
    let range = range.trim();
    let lower_inclusive = range.starts_with('[');
    let upper_inclusive = range.ends_with(']');
    if !range.starts_with(['[', '(']) || !range.ends_with([']', ')']) {
        return None;
    }

    let inner = &range[1..range.len() - 1];
    let (lo_str, hi_str) = match inner.split_once(',') {
        // `[1.0]` — an exact version, no comma.
        None => {
            let v = parse_version(inner.trim())?;
            return Some(*version == v);
        }
        Some((lo, hi)) => (lo.trim(), hi.trim()),
    };

    let lower = if lo_str.is_empty() {
        None
    } else {
        Some(Bound {
            version: parse_version(lo_str)?,
            inclusive: lower_inclusive,
        })
    };
    let upper = if hi_str.is_empty() {
        None
    } else {
        Some(Bound {
            version: parse_version(hi_str)?,
            inclusive: upper_inclusive,
        })
    };

    let above_lower = match lower {
        None => true,
        Some(b) if b.inclusive => *version >= b.version,
        Some(b) => *version > b.version,
    };
    let below_upper = match upper {
        None => true,
        Some(b) if b.inclusive => *version <= b.version,
        Some(b) => *version < b.version,
    };
    Some(above_lower && below_upper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Outcome::*;

    fn semver(req: &str, found: &str) -> Outcome {
        check(VersionSyntax::Semver, req, found)
    }
    fn maven(req: &str, found: &str) -> Outcome {
        check(VersionSyntax::Maven, req, found)
    }

    #[test]
    fn semver_the_ordinary_cases() {
        assert_eq!(semver(">=1.1.0", "1.5.0"), Satisfied);
        assert_eq!(semver(">=2.0.0", "1.5.0"), Violated);
        assert_eq!(semver("^1.2", "1.9.0"), Satisfied);
        assert_eq!(semver("^1.2", "2.0.0"), Violated);
    }

    #[test]
    fn short_and_padded_versions_still_compare() {
        assert_eq!(semver(">=0.18", "0.18"), Satisfied);
        assert_eq!(semver(">=1.0.0", "2"), Satisfied);
    }

    #[test]
    fn a_leading_v_is_not_part_of_the_number() {
        assert_eq!(semver(">=1.2.0", "v1.5.0"), Satisfied);
    }

    #[test]
    fn a_glob_pins_major_and_minor() {
        assert_eq!(semver("1.20.x", "1.20.4"), Satisfied);
        assert_eq!(semver("1.20.x", "1.21.0"), Violated);
        assert_eq!(semver("1.20.*", "1.20.0"), Satisfied);
    }

    #[test]
    fn a_glob_can_pin_only_the_major() {
        assert_eq!(semver("1.x", "1.9.9"), Satisfied);
        assert_eq!(semver("1.x", "2.0.0"), Violated);
    }

    #[test]
    fn an_unreadable_requirement_is_unverified_not_a_pass() {
        assert_eq!(semver("this is not a version", "1.0.0"), Unverified);
    }

    #[test]
    fn an_unreadable_installed_version_is_unverified() {
        assert_eq!(semver(">=1.0.0", "not-a-version"), Unverified);
    }

    #[test]
    fn maven_inclusive_lower_bound() {
        assert_eq!(maven("[36,)", "40"), Satisfied);
        assert_eq!(maven("[36,)", "36"), Satisfied);
        assert_eq!(maven("[36,)", "35"), Violated);
    }

    #[test]
    fn maven_a_closed_range() {
        assert_eq!(maven("[1.19,1.20)", "1.19.4"), Satisfied);
        assert_eq!(maven("[1.19,1.20)", "1.20.0"), Violated);
        assert_eq!(maven("[1.19,1.20)", "1.18.0"), Violated);
    }

    #[test]
    fn maven_exclusive_bounds() {
        assert_eq!(maven("(1.0,2.0)", "1.5.0"), Satisfied);
        assert_eq!(maven("(1.0,2.0)", "1.0.0"), Violated);
        assert_eq!(maven("(1.0,2.0)", "2.0.0"), Violated);
    }

    #[test]
    fn maven_an_upper_bound_only() {
        assert_eq!(maven("(,1.5]", "1.5.0"), Satisfied);
        assert_eq!(maven("(,1.5]", "1.6.0"), Violated);
    }

    #[test]
    fn maven_an_exact_version() {
        assert_eq!(maven("[1.19.2]", "1.19.2"), Satisfied);
        assert_eq!(maven("[1.19.2]", "1.19.3"), Violated);
    }

    #[test]
    fn maven_a_bare_version_is_a_soft_recommendation() {
        // Not a constraint: Maven leaves the build free to pick otherwise.
        assert_eq!(maven("1.0", "0.5"), Satisfied);
        assert_eq!(maven("1.0", "9.9"), Satisfied);
    }

    #[test]
    fn maven_or_of_several_ranges() {
        assert_eq!(maven("[1.0,2.0),[3.0,)", "1.5.0"), Satisfied);
        assert_eq!(maven("[1.0,2.0),[3.0,)", "3.1.0"), Satisfied);
        assert_eq!(maven("[1.0,2.0),[3.0,)", "2.5.0"), Violated);
    }

    #[test]
    fn maven_garbage_is_unverified() {
        assert_eq!(maven("[not,a,range)", "1.0.0"), Unverified);
    }
}
