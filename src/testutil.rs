//! Fixture builders. There is no real mod folder in the repo — tests construct
//! throwaway zips and folders so they run anywhere, offline, with no game installed.

use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;

use crate::profile;
use crate::scan::{MetadataNames, RawMod};

/// The metadata filenames every built-in profile looks for.
pub fn metadata_names() -> MetadataNames {
    profile::metadata_filenames(&profile::load_all(None).unwrap())
}

/// An inventory built in memory, without touching disk.
pub fn raw_mod(source_name: &str, entries: &[(&str, &str)]) -> RawMod {
    RawMod::from_files(PathBuf::from(source_name), entries, &metadata_names())
}

/// Write a real zip archive containing `entries` into `dir`.
pub fn write_zip_mod(dir: &Path, filename: &str, entries: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(filename);
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    for (name, content) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}

/// Write an extracted mod folder containing `entries` into `dir`.
pub fn write_folder_mod(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let root = dir.join(name);
    for (rel, content) in entries {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root
}

/// Write a real Morrowind-era `.bsa` containing empty files at `entries`.
///
/// Built with the same library that reads it back, which proves the wiring
/// rather than the format — the format itself is the library's problem, and it
/// is tested against the reference C++ suite upstream.
pub fn write_bsa(path: &Path, entries: &[&str]) {
    let mut archive = ba2::tes3::Archive::new();
    for name in entries {
        archive.insert(ba2::tes3::ArchiveKey::from(*name), ba2::tes3::File::default());
    }
    let mut out = std::fs::File::create(path).unwrap();
    archive.write(&mut out).unwrap();
}

/// A minimal valid Factorio `info.json`.
pub fn info_json(name: &str, version: &str, deps: &[&str]) -> String {
    let deps = deps
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"name":"{name}","version":"{version}","dependencies":[{deps}]}}"#)
}
