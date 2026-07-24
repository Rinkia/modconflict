//! Fixture builders. There is no real mod folder in the repo — tests construct
//! throwaway zips and folders so they run anywhere, offline, with no game installed.

use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;

use crate::scan::RawMod;

/// An inventory built in memory, without touching disk.
pub fn raw_mod(source_name: &str, entries: &[(&str, &str)]) -> RawMod {
    RawMod::from_files(PathBuf::from(source_name), entries)
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

/// A minimal valid Factorio `info.json`.
pub fn info_json(name: &str, version: &str, deps: &[&str]) -> String {
    let deps = deps
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"name":"{name}","version":"{version}","dependencies":[{deps}]}}"#)
}
