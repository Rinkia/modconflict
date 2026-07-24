//! Walks a mod folder and produces a raw inventory: what files each mod ships,
//! plus the bytes of the few metadata files the game parsers care about.
//!
//! Knows nothing about any specific game.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

/// Metadata filenames worth keeping in memory. Everything else is recorded as
/// a path only, so a folder of 500 jars does not become 500 jars of RAM.
const METADATA_FILES: &[&str] = &[
    "info.json",          // Factorio
    "fabric.mod.json",    // Minecraft (Fabric)
    "mods.toml",          // Minecraft (Forge)
    "modDesc.xml",        // Farming Simulator
];

/// One mod as found on disk, before any game-specific interpretation.
#[derive(Debug, Clone)]
pub struct RawMod {
    pub source: PathBuf,
    /// Internal paths, forward slashes, no leading slash.
    pub files: Vec<String>,
    /// Contents of the files named in `METADATA_FILES`, keyed by internal path.
    pub metadata: HashMap<String, Vec<u8>>,
}

impl RawMod {
    /// Build an inventory from an in-memory file list, applying the same
    /// metadata rule as a real scan. Used by tests to skip the filesystem.
    #[cfg(test)]
    pub fn from_files(source: PathBuf, entries: &[(&str, &str)]) -> RawMod {
        let mut files = Vec::new();
        let mut metadata = HashMap::new();
        for (path, content) in entries {
            let name = normalize(path);
            if is_metadata(&name) {
                metadata.insert(name.clone(), content.as_bytes().to_vec());
            }
            files.push(name);
        }
        files.sort();
        RawMod {
            source,
            files,
            metadata,
        }
    }

    /// First metadata file whose name matches `filename`, at any depth.
    ///
    /// A UTF-8 BOM is stripped: plenty of real mods ship one, and every parser
    /// downstream (serde_json, toml, xml) chokes on it.
    pub fn metadata_named(&self, filename: &str) -> Option<&[u8]> {
        self.metadata
            .iter()
            .find(|(path, _)| basename(path) == filename)
            .map(|(_, bytes)| strip_bom(bytes))
    }

    /// Filename of the archive or folder, for error messages and fallback ids.
    pub fn source_name(&self) -> String {
        self.source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.source.display().to_string())
    }
}

/// Scan a mod directory. Each `.zip`/`.jar` file and each subdirectory at the
/// top level is treated as one mod. Unreadable entries are skipped with a
/// warning rather than aborting the whole scan — one corrupt archive should not
/// hide the other 200 mods' conflicts.
pub fn scan_dir(dir: &Path) -> Result<Vec<RawMod>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read mod folder {}", dir.display()))?;

    let mut mods = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let result = if is_archive(&path) {
            read_archive(&path)
        } else if path.is_dir() {
            read_folder(&path)
        } else {
            continue;
        };

        match result {
            Ok(m) => mods.push(m),
            Err(e) => eprintln!("warning: skipping {}: {e:#}", path.display()),
        }
    }

    mods.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(mods)
}

fn is_archive(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "zip" | "jar")
}

fn read_archive(path: &Path) -> Result<RawMod> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;

    let mut files = Vec::new();
    let mut metadata = HashMap::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = normalize(entry.name());

        if is_metadata(&name) {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            metadata.insert(name.clone(), buf);
        }
        files.push(name);
    }

    files.sort();
    Ok(RawMod {
        source: path.to_path_buf(),
        files,
        metadata,
    })
}

fn read_folder(root: &Path) -> Result<RawMod> {
    let mut files = Vec::new();
    let mut metadata = HashMap::new();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let name = normalize(&rel.to_string_lossy());

        if is_metadata(&name) {
            metadata.insert(name.clone(), std::fs::read(entry.path())?);
        }
        files.push(name);
    }

    files.sort();
    Ok(RawMod {
        source: root.to_path_buf(),
        files,
        metadata,
    })
}

fn is_metadata(path: &str) -> bool {
    METADATA_FILES.contains(&basename(path))
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Backslashes to forward slashes, strip any leading `./` or `/`.
fn normalize(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{write_folder_mod, write_zip_mod};

    #[test]
    fn finds_zip_and_folder_mods_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(dir.path(), "alpha_1.0.0.zip", &[("alpha/data.lua", "-- a")]);
        write_folder_mod(dir.path(), "beta_1.0.0", &[("data.lua", "-- b")]);

        let mods = scan_dir(dir.path()).unwrap();

        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].files, vec!["alpha/data.lua"]);
        assert_eq!(mods[1].files, vec!["data.lua"]);
    }

    #[test]
    fn keeps_metadata_bytes_but_not_other_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(
            dir.path(),
            "alpha_1.0.0.zip",
            &[
                ("alpha_1.0.0/info.json", r#"{"name":"alpha"}"#),
                ("alpha_1.0.0/data.lua", "-- huge file"),
            ],
        );

        let mods = scan_dir(dir.path()).unwrap();

        assert_eq!(mods[0].files.len(), 2);
        assert_eq!(mods[0].metadata.len(), 1);
        assert_eq!(
            mods[0].metadata_named("info.json").unwrap(),
            br#"{"name":"alpha"}"#
        );
    }

    #[test]
    fn skips_corrupt_archive_without_losing_the_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.zip"), b"not actually a zip").unwrap();
        write_zip_mod(dir.path(), "good_1.0.0.zip", &[("good/data.lua", "-- ok")]);

        let mods = scan_dir(dir.path()).unwrap();

        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].source_name(), "good_1.0.0.zip");
    }

    #[test]
    fn strips_a_utf8_bom_from_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let with_bom = format!("\u{feff}{}", r#"{"name":"alpha"}"#);
        write_zip_mod(dir.path(), "alpha_1.0.0.zip", &[("alpha/info.json", &with_bom)]);

        let mods = scan_dir(dir.path()).unwrap();

        assert_eq!(
            mods[0].metadata_named("info.json").unwrap(),
            br#"{"name":"alpha"}"#
        );
    }

    #[test]
    fn normalizes_windows_separators() {
        assert_eq!(normalize("a\\b\\c.lua"), "a/b/c.lua");
        assert_eq!(normalize("./a/b.lua"), "a/b.lua");
    }
}
