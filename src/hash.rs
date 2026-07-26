//! Do two mods really ship a *different* file, or the same bytes twice?
//!
//! Without this, every shared path is a warning. Plenty of them are not: mod
//! packs bundle the same library, authors reupload an unchanged asset, a patch
//! ships a file it never touched. Whichever copy the game loads, it gets the
//! same file — so the warning is noise, and noise is what makes a checker get
//! ignored.
//!
//! Only the paths already flagged as overlapping are hashed. The scan itself
//! still never reads file contents, so a folder with no conflicts costs no
//! reads at all.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use crate::model::{Conflict, ModEntry};
use crate::scan::RawMod;

/// Rewrite `conflicts` in place: mark the overlaps whose copies are identical,
/// and add a `RedundantMod` wherever one mod turns out to be another's twin.
///
/// Failures are warnings, never errors. An unreadable file leaves the overlap
/// exactly as it was — reported, unresolved — which is the honest answer.
pub fn resolve_identical(
    conflicts: &mut Vec<Conflict>,
    raw: &[RawMod],
    mods: &[ModEntry],
    metadata_names: &BTreeSet<String>,
    case_sensitive: bool,
    warnings: &mut Vec<String>,
) {
    let sources: HashMap<&str, &Path> = mods
        .iter()
        .zip(raw)
        .map(|(entry, raw)| (entry.id.as_str(), raw.source.as_path()))
        .collect();
    // A mod's own spelling of each of its files, by overlap key — because a
    // reported overlap path carries only the first mod's casing, but each mod
    // must be hashed at the path it actually holds on disk.
    let real_paths: HashMap<&str, HashMap<String, &String>> = mods
        .iter()
        .map(|m| {
            let by_key = m
                .files
                .iter()
                .map(|f| (key(f, case_sensitive), f))
                .collect();
            (m.id.as_str(), by_key)
        })
        .collect();

    // Everything to hash, grouped by mod, so an archive is opened once however
    // many of its files are in conflict. Owned keys: the conflicts get rewritten
    // below, and borrowing from them would pin the list.
    let mut wanted: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for conflict in conflicts.iter() {
        if let Conflict::FileOverlap { path, mods, .. } = conflict {
            let k = key(path, case_sensitive);
            for mod_id in mods {
                if let Some(real) = real_paths.get(mod_id.as_str()).and_then(|m| m.get(&k)) {
                    wanted
                        .entry(mod_id.clone())
                        .or_default()
                        .insert((*real).clone());
                }
            }
        }
    }
    if wanted.is_empty() {
        return;
    }

    // Keyed by (mod, overlap key), so a mod's `Iron.dds` and another's
    // `iron.dds` are compared as the same slot.
    let mut digests: HashMap<(String, String), [u8; 32]> = HashMap::new();
    for (mod_id, paths) in &wanted {
        let Some(source) = sources.get(mod_id.as_str()) else {
            continue;
        };
        match hash_files(source, paths) {
            Ok(found) => {
                for (path, digest) in found {
                    digests.insert((mod_id.clone(), key(&path, case_sensitive)), digest);
                }
            }
            Err(e) => warnings.push(format!("cannot hash files in {}: {e:#}", source.display())),
        }
    }

    let mut identical_paths: HashMap<String, HashSet<String>> = HashMap::new();
    for conflict in conflicts.iter_mut() {
        let Conflict::FileOverlap {
            path,
            mods,
            identical,
            ..
        } = conflict
        else {
            continue;
        };

        let k = key(path, case_sensitive);
        let found: Vec<_> = mods
            .iter()
            .filter_map(|m| digests.get(&(m.clone(), k.clone())))
            .collect();
        // Every copy has to be readable *and* equal. A file we could not read
        // is not evidence of sameness.
        if found.len() == mods.len() && found.windows(2).all(|w| w[0] == w[1]) {
            *identical = true;
            for m in mods.iter() {
                identical_paths
                    .entry(m.clone())
                    .or_default()
                    .insert(k.clone());
            }
        }
    }

    conflicts.extend(redundant_mods(
        mods,
        &identical_paths,
        metadata_names,
        case_sensitive,
    ));
}

fn key(path: &str, case_sensitive: bool) -> String {
    crate::conflict::overlap_key(path, case_sensitive)
}

/// A mod every one of whose files is an identical copy of another mod's is not
/// a conflict — it is the same mod installed twice.
///
/// Reported once per pair. Two true twins would otherwise each accuse the
/// other, which says the same thing twice and reads like two problems.
fn redundant_mods(
    mods: &[ModEntry],
    identical_paths: &HashMap<String, HashSet<String>>,
    metadata_names: &BTreeSet<String>,
    case_sensitive: bool,
) -> Vec<Conflict> {
    // The manifest is excluded: it necessarily differs, because it carries the
    // id, and demanding it match would make redundancy undetectable. Files are
    // held by overlap key so two mods are compared the way the game sees them.
    let owned: HashMap<&str, HashSet<String>> = mods
        .iter()
        .map(|m| {
            let keys = m
                .files
                .iter()
                .filter(|f| !crate::conflict::is_boring(f, metadata_names))
                .map(|f| key(f, case_sensitive))
                .collect();
            (m.id.as_str(), keys)
        })
        .collect();

    let mut out = Vec::new();
    for entry in mods {
        let Some(identical) = identical_paths.get(&entry.id) else {
            continue;
        };
        let mine = &owned[entry.id.as_str()];
        if mine.is_empty() || mine.iter().any(|f| !identical.contains(f)) {
            continue;
        }

        let twin = mods
            .iter()
            .filter(|other| other.id != entry.id)
            .filter(|other| {
                let theirs = &owned[other.id.as_str()];
                mine.iter().all(|f| theirs.contains(f))
            })
            // A mod contained in a larger one is the redundant half. Between
            // true twins, the later id reports and the earlier is named, so
            // the pair is stated once.
            .filter(|other| owned[other.id.as_str()].len() > mine.len() || other.id < entry.id)
            .map(|other| other.id.as_str())
            .min();

        if let Some(duplicate_of) = twin {
            out.push(Conflict::RedundantMod {
                mod_id: entry.id.clone(),
                duplicate_of: duplicate_of.to_string(),
                files: mine.len(),
            });
        }
    }
    out
}

/// Hash the named paths inside one mod, whether it is a folder or an archive.
///
/// Paths that cannot be found are simply absent from the result, which the
/// caller reads as "not proven identical".
fn hash_files(source: &Path, paths: &HashSet<String>) -> anyhow::Result<Vec<(String, [u8; 32])>> {
    let mut out = Vec::new();

    if source.is_dir() {
        for path in paths {
            let full = source.join(path);
            if let Ok(file) = std::fs::File::open(&full) {
                out.push((path.clone(), hash_reader(file)?));
            }
        }
        return Ok(out);
    }

    // Anything else is an archive. Only zip contents are reachable: a binary
    // container gives up its file list but not its bytes, so those overlaps
    // stay unresolved rather than being guessed at.
    let Ok(file) = std::fs::File::open(source) else {
        return Ok(out);
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return Ok(out);
    };
    for path in paths {
        if let Ok(entry) = zip.by_name(path) {
            out.push((path.clone(), hash_reader(entry)?));
        }
    }
    Ok(out)
}

/// Streamed, so a 300 MB texture costs a buffer and not a copy of itself.
fn hash_reader<R: Read>(mut reader: R) -> anyhow::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{self, Options};

    fn fabric_mod(dir: &Path, name: &str, texture: &str) {
        crate::testutil::write_folder_mod(
            dir,
            name,
            &[
                (
                    "fabric.mod.json",
                    &format!(r#"{{"id":"{name}","version":"1.0.0"}}"#),
                ),
                ("assets/stone.png", texture),
            ],
        );
    }

    fn analyze(dir: &Path, hash: bool) -> Vec<Conflict> {
        analyze::run(
            dir,
            &Options {
                skip_hashing: !hash,
                ..Default::default()
            },
        )
        .unwrap()
        .conflicts
    }

    #[test]
    fn two_mods_shipping_the_same_bytes_are_only_worth_a_note() {
        let dir = tempfile::tempdir().unwrap();
        fabric_mod(dir.path(), "alpha", "the same bytes");
        fabric_mod(dir.path(), "beta", "the same bytes");

        let conflicts = analyze(dir.path(), true);

        let overlap = conflicts
            .iter()
            .find(|c| matches!(c, Conflict::FileOverlap { .. }))
            .unwrap();
        assert!(matches!(
            overlap,
            Conflict::FileOverlap {
                identical: true,
                ..
            }
        ));
        assert_eq!(overlap.severity(), crate::model::Severity::Info);
        assert!(overlap.title().contains("identical"));
    }

    #[test]
    fn identical_bytes_under_different_case_are_still_recognised() {
        // The overlap collides case-insensitively; hashing must then find each
        // mod's own spelling on disk and compare the bytes.
        let dir = tempfile::tempdir().unwrap();
        crate::testutil::write_folder_mod(
            dir.path(),
            "alpha",
            &[
                ("fabric.mod.json", r#"{"id":"alpha","version":"1.0.0"}"#),
                ("assets/Stone.png", "identical bytes"),
            ],
        );
        crate::testutil::write_folder_mod(
            dir.path(),
            "beta",
            &[
                ("fabric.mod.json", r#"{"id":"beta","version":"1.0.0"}"#),
                ("assets/stone.png", "identical bytes"),
            ],
        );

        let conflicts = analyze(dir.path(), true);

        assert!(
            conflicts.iter().any(|c| matches!(
                c,
                Conflict::FileOverlap {
                    identical: true,
                    ..
                }
            )),
            "{conflicts:#?}"
        );
    }

    #[test]
    fn two_mods_shipping_different_bytes_still_warn() {
        let dir = tempfile::tempdir().unwrap();
        fabric_mod(dir.path(), "alpha", "one thing");
        fabric_mod(dir.path(), "beta", "something else");

        let conflicts = analyze(dir.path(), true);

        let overlap = conflicts
            .iter()
            .find(|c| matches!(c, Conflict::FileOverlap { .. }))
            .unwrap();
        assert!(matches!(
            overlap,
            Conflict::FileOverlap {
                identical: false,
                ..
            }
        ));
        assert_eq!(overlap.severity(), crate::model::Severity::Warning);
    }

    #[test]
    fn without_hashing_nothing_is_claimed_to_be_identical() {
        let dir = tempfile::tempdir().unwrap();
        fabric_mod(dir.path(), "alpha", "the same bytes");
        fabric_mod(dir.path(), "beta", "the same bytes");

        let conflicts = analyze(dir.path(), false);

        assert!(conflicts.iter().any(|c| matches!(
            c,
            Conflict::FileOverlap {
                identical: false,
                ..
            }
        )));
        assert!(!conflicts
            .iter()
            .any(|c| matches!(c, Conflict::RedundantMod { .. })));
    }

    #[test]
    fn a_mod_that_is_a_copy_of_another_is_named_as_redundant() {
        let dir = tempfile::tempdir().unwrap();
        // Identical but for the manifest, which must differ: it carries the id.
        for name in ["original", "duplicate"] {
            crate::testutil::write_folder_mod(
                dir.path(),
                name,
                &[
                    (
                        "fabric.mod.json",
                        &format!(r#"{{"id":"{name}","version":"1.0.0"}}"#),
                    ),
                    ("assets/stone.png", "same"),
                    ("assets/wood.png", "same too"),
                ],
            );
        }

        let conflicts = analyze(dir.path(), true);

        let redundant: Vec<_> = conflicts
            .iter()
            .filter(|c| matches!(c, Conflict::RedundantMod { .. }))
            .collect();
        assert!(!redundant.is_empty(), "{conflicts:#?}");
        assert_eq!(redundant[0].severity(), crate::model::Severity::Info);
    }

    #[test]
    fn a_mod_sharing_only_some_files_is_not_redundant() {
        let dir = tempfile::tempdir().unwrap();
        crate::testutil::write_folder_mod(
            dir.path(),
            "small",
            &[
                ("fabric.mod.json", r#"{"id":"small","version":"1.0.0"}"#),
                ("assets/stone.png", "same"),
            ],
        );
        crate::testutil::write_folder_mod(
            dir.path(),
            "big",
            &[
                ("fabric.mod.json", r#"{"id":"big","version":"1.0.0"}"#),
                ("assets/stone.png", "same"),
                ("assets/unique.png", "only here"),
            ],
        );

        let conflicts = analyze(dir.path(), true);

        // "small" is contained in "big", so it is redundant; "big" is not.
        let redundant: Vec<&str> = conflicts
            .iter()
            .filter_map(|c| match c {
                Conflict::RedundantMod { mod_id, .. } => Some(mod_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(redundant, vec!["small"]);
    }

    #[test]
    fn identical_files_inside_archives_are_recognised_too() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["alpha", "beta"] {
            crate::testutil::write_zip_mod(
                dir.path(),
                &format!("{name}.jar"),
                &[
                    (
                        "fabric.mod.json",
                        &format!(r#"{{"id":"{name}","version":"1.0.0"}}"#),
                    ),
                    ("assets/stone.png", "identical bytes"),
                ],
            );
        }

        let conflicts = analyze(dir.path(), true);

        assert!(conflicts.iter().any(|c| matches!(
            c,
            Conflict::FileOverlap {
                identical: true,
                ..
            }
        )));
    }

    #[test]
    fn hashing_the_same_bytes_twice_agrees_with_itself() {
        let a = hash_reader(&b"some bytes"[..]).unwrap();
        let b = hash_reader(&b"some bytes"[..]).unwrap();
        let c = hash_reader(&b"other bytes"[..]).unwrap();

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hashing_streams_rather_than_buffering_the_whole_file() {
        // Larger than the internal buffer, so this exercises the loop.
        let big = vec![b'x'; 200 * 1024];
        assert_eq!(
            hash_reader(&big[..]).unwrap(),
            hash_reader(&big[..]).unwrap()
        );
    }
}
