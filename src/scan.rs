//! Walks a mod folder and produces a raw inventory: what files each mod ships,
//! plus the bytes of the few metadata files the game parsers care about.
//!
//! Knows nothing about any specific game.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::container;
use crate::limits;
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
/// What a scan found, plus everything it could not read.
///
/// Warnings are returned rather than printed so they reach the JSON report and
/// so a test can assert that a limit actually fired.
#[derive(Debug, Default)]
pub struct Scan {
    pub mods: Vec<RawMod>,
    pub warnings: Vec<String>,
}

pub fn scan_dir(dir: &Path, names: &MetadataNames) -> Result<Scan> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read mod folder {}", dir.display()))?;

    let mut scan = Scan::default();
    for entry in entries {
        let path = entry?.path();
        let result = if is_archive(&path) {
            read_archive(&path, names, &mut scan.warnings)
        } else if path.is_dir() {
            read_folder(&path, names, &mut scan.warnings)
        } else if container::is_candidate(&path) {
            // A .pak or .bsa lying directly in the mods folder is the mod, not
            // a file inside one — which is how BG3 and most Unreal games ship.
            read_container(&path, &mut scan.warnings)
        } else {
            continue;
        };

        match result {
            Ok(m) => scan.mods.push(m),
            Err(e) => scan
                .warnings
                .push(format!("skipping {}: {e:#}", path.display())),
        }
    }

    scan.mods.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(scan)
}

fn is_archive(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "zip" | "jar")
}

fn read_archive(path: &Path, names: &MetadataNames, warnings: &mut Vec<String>) -> Result<RawMod> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;

    let mut files = Vec::new();
    let mut metadata = HashMap::new();
    let mut containers = Vec::new();

    let entry_count = zip.len().min(limits::MAX_ARCHIVE_ENTRIES);
    if zip.len() > limits::MAX_ARCHIVE_ENTRIES {
        warnings.push(format!(
            "{}: claims {} entries, reading the first {}",
            path.display(),
            zip.len(),
            limits::MAX_ARCHIVE_ENTRIES
        ));
    }

    for i in 0..entry_count {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = normalize(entry.name());

        if is_metadata(&name, names) {
            match metadata_refusal(entry.size(), entry.compressed_size()) {
                Some(reason) => {
                    warnings.push(format!("{}: skipping {name}: {reason}", path.display()))
                }
                None => {
                    // The read is bounded as well as the check: a header that
                    // lies about its size must not become an allocation.
                    let mut buf = Vec::new();
                    entry
                        .by_ref()
                        .take(limits::MAX_METADATA_BYTES)
                        .read_to_end(&mut buf)?;
                    metadata.insert(name.clone(), buf);
                }
            }
        } else if container::is_candidate(Path::new(&name)) {
            // A binary archive packed inside the zip (a .bsa inside a .zip is
            // common) contributes the paths inside it, exactly like a loose one
            // in a folder mod. The container readers work on files, so it is
            // extracted to a temp file first — guarded against a decompression
            // bomb and capped so one mod cannot fill the disk.
            if limits::is_decompression_bomb(entry.size(), entry.compressed_size()) {
                warnings.push(format!(
                    "{}: skipping nested {name}: looks like a decompression bomb",
                    path.display()
                ));
            } else if entry.size() > limits::MAX_NESTED_CONTAINER_BYTES {
                warnings.push(format!(
                    "{}: skipping nested {name}: {} bytes is too large to expand",
                    path.display(),
                    entry.size()
                ));
            } else {
                let ext = name.rsplit('.').next().unwrap_or("");
                let bounded = entry.by_ref().take(limits::MAX_NESTED_CONTAINER_BYTES);
                match read_nested_container(bounded, ext) {
                    Ok(Some(mut archive)) => {
                        truncate_container(path, &mut archive, warnings);
                        containers.push(archive.format);
                        files.extend(archive.files);
                    }
                    // The extension matched but it was not really a container.
                    Ok(None) => {}
                    Err(e) => warnings.push(format!(
                        "{}: cannot read nested {name}: {e:#}",
                        path.display()
                    )),
                }
            }
        }
        files.push(name);
    }

    files.sort();
    Ok(RawMod {
        source: path.to_path_buf(),
        files,
        metadata,
        containers,
    })
}

/// Extract an archive entry to a temp file (the container readers need a path)
/// and read the file list out of it. The temp file carries the original
/// extension so the reader sniffs the right format.
fn read_nested_container(mut reader: impl Read, ext: &str) -> Result<Option<container::Container>> {
    let mut temp = tempfile::Builder::new()
        .suffix(&format!(".{ext}"))
        .tempfile()?;
    std::io::copy(&mut reader, temp.as_file_mut())?;
    temp.as_file_mut().flush()?;
    container::read(temp.path())
}

fn read_container(path: &Path, warnings: &mut Vec<String>) -> Result<RawMod> {
    let mut archive =
        container::read(path)?.ok_or_else(|| anyhow::anyhow!("not a container after all"))?;
    truncate_container(path, &mut archive, warnings);

    Ok(RawMod {
        source: path.to_path_buf(),
        files: archive.files,
        metadata: HashMap::new(),
        containers: vec![archive.format],
    })
}

/// Why a metadata entry must not be read, if it must not.
fn metadata_refusal(uncompressed: u64, compressed: u64) -> Option<String> {
    if limits::is_decompression_bomb(uncompressed, compressed) {
        return Some(format!(
            "{uncompressed} bytes from {compressed} compressed looks like a decompression bomb"
        ));
    }
    (uncompressed > limits::MAX_METADATA_BYTES).then(|| {
        format!(
            "{uncompressed} bytes is too large for a metadata file (limit {})",
            limits::MAX_METADATA_BYTES
        )
    })
}

fn truncate_container(path: &Path, archive: &mut container::Container, warnings: &mut Vec<String>) {
    if archive.files.len() > limits::MAX_CONTAINER_ENTRIES {
        warnings.push(format!(
            "{}: archive lists {} entries, keeping the first {}",
            path.display(),
            archive.files.len(),
            limits::MAX_CONTAINER_ENTRIES
        ));
        archive.files.truncate(limits::MAX_CONTAINER_ENTRIES);
    }
}

fn read_folder(root: &Path, names: &MetadataNames, warnings: &mut Vec<String>) -> Result<RawMod> {
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
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            match metadata_refusal(size, size) {
                Some(reason) => warnings.push(format!("{}: {reason}", entry.path().display())),
                None => {
                    metadata.insert(name.clone(), std::fs::read(entry.path())?);
                }
            }
        }
        // A binary archive contributes the paths inside it, not just its own
        // name: that is what makes a texture inside a .bsa collide with a loose
        // copy of the same texture, which is how the game sees it too.
        match container::read(entry.path()) {
            Ok(Some(mut archive)) => {
                truncate_container(entry.path(), &mut archive, warnings);
                containers.push(archive.format);
                files.extend(archive.files);
            }
            Ok(None) => {}
            Err(e) => warnings.push(format!(
                "cannot read archive {}: {e:#}",
                entry.path().display()
            )),
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

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap().mods;

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

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap().mods;

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

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap().mods;

        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].source_name(), "good_1.0.0.zip");
    }

    #[test]
    fn a_binary_container_inside_a_zip_is_expanded() {
        use crate::testutil::write_bsa;

        let dir = tempfile::tempdir().unwrap();

        // Build a real .bsa, grab its bytes, then pack it into a .zip mod.
        let bsa = dir.path().join("scratch.bsa");
        write_bsa(&bsa, &["textures/armor/iron.dds", "meshes/x.nif"]);
        let bsa_bytes = std::fs::read(&bsa).unwrap();
        std::fs::remove_file(&bsa).unwrap();

        let zip_path = dir.path().join("packed_1.0.0.zip");
        let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("Packed.bsa", opts).unwrap();
        w.write_all(&bsa_bytes).unwrap();
        w.start_file("readme.txt", opts).unwrap();
        w.write_all(b"hi").unwrap();
        w.finish().unwrap();

        let mods = scan_dir(dir.path(), &metadata_names()).unwrap().mods;

        assert_eq!(mods.len(), 1);
        // The .bsa's internal paths are now the mod's files, so a texture inside
        // it can collide with a loose copy elsewhere.
        assert!(mods[0].files.iter().any(|f| f == "textures/armor/iron.dds"));
        assert!(mods[0].files.iter().any(|f| f == "meshes/x.nif"));
        // The .bsa itself is still listed alongside its contents.
        assert!(mods[0].files.iter().any(|f| f == "Packed.bsa"));
        assert!(mods[0].containers.contains(&"Bethesda archive"));
    }

    #[test]
    fn normalizes_windows_separators() {
        assert_eq!(normalize("a\\b\\c.lua"), "a/b/c.lua");
        assert_eq!(normalize("./a/b.lua"), "a/b.lua");
    }
}
