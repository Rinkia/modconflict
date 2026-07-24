//! Walks a mod folder and produces a raw inventory: what files each mod ships,
//! plus the bytes of the few metadata files the game parsers care about.
//!
//! Knows nothing about any specific game.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::container;
use walkdir::WalkDir;

/// The metadata filenames to keep in memory, which the profiles decide.
/// Everything else is recorded as a path only, so a folder of 500 jars does not
/// become 500 jars of RAM.
pub type MetadataNames = BTreeSet<String>;

/// One mod as found on disk, before any game-specific interpretation.
#[derive(Debug, Clone)]
pub struct RawMod {
    pub source: PathBuf,
    /// Internal paths, forward slashes, no leading slash.
    pub files: Vec<String>,
    /// Contents of the metadata files, keyed by internal path.
    pub metadata: HashMap<String, Vec<u8>>,
    /// Binary archives inside this mod whose contents were expanded into
    /// `files`, by format name.
    pub containers: Vec<&'static str>,
}

impl RawMod {
    /// Build an inventory from an in-memory file list, applying the same
    /// metadata rule as a real scan. Used by tests to skip the filesystem.
    #[cfg(test)]
    pub fn from_files(source: PathBuf, entries: &[(&str, &str)], names: &MetadataNames) -> RawMod {
        let mut files = Vec::new();
        let mut metadata = HashMap::new();
        for (path, content) in entries {
            let name = normalize(path);
            if is_metadata(&name, names) {
                metadata.insert(name.clone(), content.as_bytes().to_vec());
            }
            files.push(name);
        }
        files.sort();
        RawMod {
            source,
            files,
            metadata,
            containers: Vec::new(),
        }
    }

    /// First metadata file whose name matches `filename`, at any depth.
    pub fn metadata_named(&self, filename: &str) -> Option<&[u8]> {
        self.metadata
            .iter()
            .find(|(path, _)| basename(path) == filename)
            .map(|(_, bytes)| bytes.as_slice())
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
pub fn scan_dir(dir: &Path, names: &MetadataNames) -> Result<Vec<RawMod>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read mod folder {}", dir.display()))?;

    let mut mods = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let result = if is_archive(&path) {
            read_archive(&path, names)
        } else if path.is_dir() {
            read_folder(&path, names)
        } else if container::is_candidate(&path) {
            // A .pak or .bsa lying directly in the mods folder is the mod, not
            // a file inside one — which is how BG3 and most Unreal games ship.
            read_container(&path)
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

fn read_archive(path: &Path, names: &MetadataNames) -> Result<RawMod> {
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

        if is_metadata(&name, names) {
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
        containers: Vec::new(),
    })
}

fn read_container(path: &Path) -> Result<RawMod> {
    let archive = container::read(path)?
        .ok_or_else(|| anyhow::anyhow!("not a container after all"))?;

    Ok(RawMod {
        source: path.to_path_buf(),
        files: archive.files,
        metadata: HashMap::new(),
        containers: vec![archive.format],
    })
}

fn read_folder(root: &Path, names: &MetadataNames) -> Result<RawMod> {
    let mut files = Vec::new();
    let mut metadata = HashMap::new();
    let mut containers = Vec::new();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let name = normalize(&rel.to_string_lossy());

        if is_metadata(&name, names) {
            metadata.insert(name.clone(), std::fs::read(entry.path())?);
        }
        // A binary archive contributes the paths inside it, not just its own
        // name: that is what makes a texture inside a .bsa collide with a loose
        // copy of the same texture, which is how the game sees it too.
        match container::read(entry.path()) {
            Ok(Some(archive)) => {
                containers.push(archive.format);
                files.extend(archive.files);
            }
            Ok(None) => {}
            Err(e) => eprintln!(
                "warning: cannot read archive {}: {e:#}",
                entry.path().display()
            ),
        }
        files.push(name);
    }

    files.sort();
    Ok(RawMod {
        source: root.to_path_buf(),
        files,
        metadata,
        containers,
    })
}

fn is_metadata(path: &str, names: &MetadataNames) -> bool {
    names.contains(basename(path))
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
    use crate::testutil::{metadata_names, write_folder_mod, write_zip_mod};

    #[test]
    fn finds_zip_and_folder_mods_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        write_zip_mod(dir.path(), "alpha_1.0.0.zip", &[("alpha/data.lua", "-- a")]);
        write_folder_mod(dir.path(), "beta_1.0.0", &[("data.lua", "-- b")]);

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap();

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

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap();

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

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap();

        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].source_name(), "good_1.0.0.zip");
    }

    #[test]
    fn normalizes_windows_separators() {
        assert_eq!(normalize("a\\b\\c.lua"), "a/b/c.lua");
        assert_eq!(normalize("./a/b.lua"), "a/b.lua");
    }
}
