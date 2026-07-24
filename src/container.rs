//! Binary mod containers.
//!
//! A profile can describe text metadata, but it cannot describe a proprietary
//! binary archive — so those need code. What is general here is not a byte-level
//! description language (that road ends at "write a parser in TOML"); it is the
//! *contract*: a container reader takes a file and returns the paths inside it.
//!
//! That single answer is enough to make binary archives participate in every
//! check the detector already does. A texture inside a Skyrim `.bsa` and a
//! loose copy of the same texture now collide in the report exactly the way
//! they collide in the game.
//!
//! The parsers themselves are not ours: they are maintained crates that already
//! track each format's version drift, which is the part that actually rots.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// A reader for one family of binary archive.
struct ContainerFormat {
    /// Name shown in reports.
    name: &'static str,
    /// True when the first bytes of a file look like this format. Cheap, and
    /// wrong extensions are common, so sniffing beats trusting the filename.
    sniff: fn(&[u8]) -> bool,
    /// Full read: the paths inside the archive.
    read: fn(&Path) -> anyhow::Result<Vec<String>>,
}

const FORMATS: &[ContainerFormat] = &[
    ContainerFormat {
        // Morrowind through Starfield: .bsa and .ba2
        name: "Bethesda archive",
        sniff: sniff_bethesda,
        read: read_bethesda,
    },
    ContainerFormat {
        // Source engine: .vpk
        name: "Valve pak",
        sniff: sniff_vpk,
        read: read_vpk,
    },
    ContainerFormat {
        // Unreal Engine 4 and 5: .pak
        name: "Unreal pak",
        sniff: sniff_unreal,
        read: read_unreal,
    },
];

/// How many bytes to sniff. All supported formats decide within this.
const SNIFF_LEN: usize = 8;

/// Only files with one of these extensions are opened at all. Sniffing every
/// file in a mod folder would cost a syscall per texture for no gain, and one
/// of the readers below has to be permissive about headers — bounding it to
/// plausible filenames keeps that permissiveness harmless.
const CONTAINER_EXTENSIONS: &[&str] = &["bsa", "ba2", "vpk", "pak"];

pub fn is_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| CONTAINER_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub struct Container {
    pub format: &'static str,
    /// Paths inside the archive, forward slashes, lowercase-preserved.
    pub files: Vec<String>,
}

/// Read the file list out of `path` when it is a container we understand.
///
/// `Ok(None)` means "not a container", which is the common case and not a
/// problem. `Err` means it looked like one and could not be read — worth a
/// warning, but never fatal: one unreadable archive must not hide the rest of
/// the report.
pub fn read(path: &Path) -> anyhow::Result<Option<Container>> {
    if !is_candidate(path) {
        return Ok(None);
    }
    let Some(format) = sniff(path)? else {
        return Ok(None);
    };

    let files = (format.read)(path)?;
    Ok(Some(Container {
        format: format.name,
        files: files.into_iter().map(|f| normalize(&f)).collect(),
    }))
}

fn sniff(path: &Path) -> anyhow::Result<Option<&'static ContainerFormat>> {
    let mut head = [0u8; SNIFF_LEN];
    let mut file = File::open(path)?;
    let read = file.read(&mut head)?;
    let head = &head[..read];

    Ok(FORMATS.iter().find(|f| (f.sniff)(head)))
}

/// `BSA\0` (Oblivion onwards), `BTDX` (Fallout 4 onwards), or Morrowind's
/// version-number-as-magic `0x100`.
fn sniff_bethesda(head: &[u8]) -> bool {
    head.starts_with(b"BSA\0") || head.starts_with(b"BTDX") || head.starts_with(&0x100u32.to_le_bytes())
}

fn read_bethesda(path: &Path) -> anyhow::Result<Vec<String>> {
    use ba2::prelude::*;

    let mut file = File::open(path)?;
    let format = ba2::guess_format(&mut file)
        .ok_or_else(|| anyhow::anyhow!("not a recognized Bethesda archive"))?;

    let mut files = Vec::new();
    match format {
        ba2::FileFormat::TES3 => {
            let archive = ba2::tes3::Archive::read(path)?;
            for (key, _) in &archive {
                files.push(key.name().to_string());
            }
        }
        ba2::FileFormat::TES4 => {
            let (archive, _) = ba2::tes4::Archive::read(path)?;
            for (directory_key, directory) in &archive {
                let dir = directory_key.name().to_string();
                for (file_key, _) in directory {
                    files.push(format!("{dir}/{}", file_key.name()));
                }
            }
        }
        ba2::FileFormat::FO4 => {
            let (archive, _) = ba2::fo4::Archive::read(path)?;
            for (key, _) in &archive {
                files.push(key.name().to_string());
            }
        }
    }
    Ok(files)
}

/// `0x55AA1234`, little endian.
fn sniff_vpk(head: &[u8]) -> bool {
    head.starts_with(&0x55AA_1234u32.to_le_bytes())
}

fn read_vpk(path: &Path) -> anyhow::Result<Vec<String>> {
    let archive = vpk::from_path(path).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(archive.tree.into_keys().collect())
}

/// Unreal keeps its magic in a *footer*, not a header — a pak begins with raw
/// packed data. So there is nothing to sniff: this entry runs last, claims any
/// `.pak` no earlier format claimed, and lets the reader have the final say.
fn sniff_unreal(head: &[u8]) -> bool {
    head.len() >= 4 && !head.starts_with(b"PK\x03\x04")
}

fn read_unreal(path: &Path) -> anyhow::Result<Vec<String>> {
    // Only a real pak has the footer magic; anything else errors out here,
    // which is exactly the "not a container after all" signal.
    let pak = unpak::Pak::new_any(path, None).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(pak.entries())
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    #[test]
    fn recognizes_the_bethesda_magics() {
        assert!(sniff_bethesda(b"BSA\0\x68\0\0\0"));
        assert!(sniff_bethesda(b"BTDX\x01\0\0\0"));
        assert!(sniff_bethesda(&0x100u32.to_le_bytes()));
        assert!(!sniff_bethesda(b"PK\x03\x04"));
    }

    #[test]
    fn recognizes_the_valve_magic() {
        assert!(sniff_vpk(&0x55AA_1234u32.to_le_bytes()));
        assert!(!sniff_vpk(b"BSA\0"));
    }

    #[test]
    fn a_zip_is_never_claimed_as_a_binary_container() {
        let dir = tempfile::tempdir().unwrap();
        // Real zip header; zips are handled by the scanner, not here.
        let path = write_file(dir.path(), "mod.zip", b"PK\x03\x04\x14\0\0\0");

        assert!(sniff(&path).unwrap().is_none());
    }

    #[test]
    fn a_file_too_short_to_identify_is_not_a_container() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "tiny.dat", b"ab");

        assert!(sniff(&path).unwrap().is_none());
    }

    #[test]
    fn a_truncated_bethesda_archive_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        // Correct magic, nothing behind it.
        let path = write_file(dir.path(), "broken.bsa", b"BSA\0\x68\0\0\0");

        assert!(read(&path).is_err());
    }

    #[test]
    fn a_file_that_only_looks_like_an_unreal_pak_fails_to_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "notreally.pak", b"random bytes here");

        // Claimed by the permissive Unreal sniff, then rejected by the reader.
        assert!(read(&path).is_err());
    }

    #[test]
    fn normalizes_separators_in_archive_paths() {
        assert_eq!(normalize("textures\\armor\\iron.dds"), "textures/armor/iron.dds");
        assert_eq!(normalize("/meshes/x.nif"), "meshes/x.nif");
    }
}
