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
        archive.insert(
            ba2::tes3::ArchiveKey::from(*name),
            ba2::tes3::File::default(),
        );
    }
    let mut out = std::fs::File::create(path).unwrap();
    archive.write(&mut out).unwrap();
}

/// Write a minimal but genuine Creation Engine plugin: a `TES4` header record
/// declaring `masters`, then one group holding one record per entry in
/// `form_ids`.
///
/// Hand-built bytes, but not unverified: `esplugin` is the authority that reads
/// them back, and it rejects anything malformed — a test that gets the layout
/// wrong fails rather than quietly passing.
#[cfg_attr(not(feature = "records"), allow(dead_code))]
pub fn write_plugin(path: &Path, masters: &[&str], form_ids: &[u32]) {
    const RECORD_HEADER_LEN: u32 = 24;

    let mut header_data = Vec::new();
    // HEDR: version, record count, next object id.
    header_data.extend(subrecord(b"HEDR", &{
        let mut d = Vec::new();
        d.extend(1.7f32.to_le_bytes());
        d.extend((form_ids.len() as i32).to_le_bytes());
        d.extend(0x800u32.to_le_bytes());
        d
    }));
    for master in masters {
        let mut name = master.as_bytes().to_vec();
        name.push(0); // zstring
        header_data.extend(subrecord(b"MAST", &name));
        header_data.extend(subrecord(b"DATA", &0u64.to_le_bytes()));
    }

    let mut out = Vec::new();
    out.extend(record_header(b"TES4", header_data.len() as u32, 0));
    out.extend(&header_data);

    if !form_ids.is_empty() {
        let mut records = Vec::new();
        for id in form_ids {
            // A record with no subrecords: the FormID in its header is all the
            // overlap check needs.
            records.extend(record_header(b"WEAP", 0, *id));
        }
        out.extend(b"GRUP");
        out.extend((RECORD_HEADER_LEN + records.len() as u32).to_le_bytes());
        out.extend([0u8; 16]); // label, group type, timestamp, version
        out.extend(&records);
    }

    std::fs::write(path, out).unwrap();
}

#[cfg_attr(not(feature = "records"), allow(dead_code))]
fn record_header(kind: &[u8; 4], data_len: u32, form_id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend(kind);
    out.extend(data_len.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // flags
    out.extend(form_id.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // version control
    out.extend(0u32.to_le_bytes()); // form version + unknown
    out
}

#[cfg_attr(not(feature = "records"), allow(dead_code))]
fn subrecord(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + data.len());
    out.extend(kind);
    out.extend((data.len() as u16).to_le_bytes());
    out.extend(data);
    out
}

/// Build a real `.pak` with the same library that reads it back, so the test
/// exercises the wiring rather than restating the format.
///
/// The loose source tree is staged in its own temp directory: left in `root` it
/// would be scanned as a second, folder-shaped mod alongside the pak.
pub fn write_bg3_pak(root: &Path, folder: &str, meta: &str) -> PathBuf {
    let staging = tempfile::tempdir().unwrap();
    let mod_root = staging.path().join(folder);
    let meta_dir = mod_root.join("Mods").join(folder);
    std::fs::create_dir_all(&meta_dir).unwrap();
    std::fs::write(meta_dir.join("meta.lsx"), meta).unwrap();

    // The crate resolves a None destination to a bare relative filename, which
    // lands in the process CWD rather than next to the mod.
    let out = root.join(format!("{folder}.pak"));
    larian_formats::lspk::write(&mod_root, Some(out.clone())).unwrap();
    out
}

pub fn bg3_meta_lsx(uuid: &str, name: &str, version: i64, deps: &[(&str, &str)]) -> String {
    let dependencies: String = deps
        .iter()
        .map(|(dep_uuid, dep_name)| {
            format!(
                r#"<node id="ModuleShortDesc">
                     <attribute id="Folder" type="LSString" value="{dep_name}"/>
                     <attribute id="Name" type="LSString" value="{dep_name}"/>
                     <attribute id="UUID" type="FixedString" value="{dep_uuid}"/>
                     <attribute id="Version64" type="int64" value="36028797018963968"/>
                   </node>"#
            )
        })
        .collect();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<save>
  <version major="4" minor="0" revision="9" build="331"/>
  <region id="Config">
<node id="root">
  <children>
    <node id="Dependencies">
      <children>{dependencies}</children>
    </node>
    <node id="ModuleInfo">
      <attribute id="Author" type="LSString" value="Someone"/>
      <attribute id="Name" type="LSString" value="{name}"/>
      <attribute id="Folder" type="LSString" value="{name}"/>
      <attribute id="UUID" type="FixedString" value="{uuid}"/>
      <attribute id="Version64" type="int64" value="{version}"/>
    </node>
  </children>
</node>
  </region>
</save>"#
    )
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
