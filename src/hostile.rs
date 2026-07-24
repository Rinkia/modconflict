//! Hostile input.
//!
//! Every file this tool opens was downloaded from the internet by someone who
//! wanted a nicer sword. These tests feed it the shapes an attacker or a
//! corrupted download produces, and assert the same thing every time: an error
//! or a warning, never a panic, never an unbounded allocation, and never a
//! silent success that hides the rest of the folder.
//!
//! Fuzzing finds the cases nobody thought of; this file pins the ones we did.

#![cfg(test)]

use std::io::Write;
use std::path::Path;

use crate::analyze::{self, Options};
use crate::limits;
use crate::scan;
use crate::testutil::metadata_names;
use crate::value::{self, Format};

fn scan_dir(dir: &Path) -> scan::Scan {
    scan::scan_dir(dir, &metadata_names()).unwrap()
}

/// A zip whose entry declares a gigabyte but holds a handful of bytes.
fn write_lying_zip(path: &Path) {
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("mod/info.json", SimpleFileOptions::default())
        .unwrap();
    // Highly compressible: a real ratio far past the bomb threshold.
    let payload = vec![b'a'; 12 * 1024 * 1024];
    zip.write_all(&payload).unwrap();
    zip.finish().unwrap();
}

#[test]
fn a_decompression_bomb_is_skipped_with_a_warning_not_expanded() {
    let dir = tempfile::tempdir().unwrap();
    write_lying_zip(&dir.path().join("bomb.zip"));

    let scan = scan_dir(dir.path());

    // The mod still exists and still lists its files.
    assert_eq!(scan.mods.len(), 1);
    assert!(scan.mods[0].files.iter().any(|f| f == "mod/info.json"));
    // But its contents were never read into memory.
    assert!(scan.mods[0].metadata.is_empty());
    assert!(
        scan.warnings.iter().any(|w| w.contains("decompression bomb")
            || w.contains("too large for a metadata file")),
        "{:?}",
        scan.warnings
    );
}

#[test]
fn an_oversized_loose_metadata_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mod_dir = dir.path().join("HugeMod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(
        mod_dir.join("info.json"),
        vec![b'x'; limits::MAX_METADATA_BYTES as usize + 1],
    )
    .unwrap();

    let scan = scan_dir(dir.path());

    assert!(scan.mods[0].metadata.is_empty());
    assert!(
        scan.warnings.iter().any(|w| w.contains("too large")),
        "{:?}",
        scan.warnings
    );
}

#[test]
fn a_truncated_archive_costs_one_mod_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    // A valid local file header, then nothing.
    std::fs::write(dir.path().join("broken.zip"), b"PK\x03\x04\x14\0\0\0\0\0").unwrap();
    crate::testutil::write_zip_mod(
        dir.path(),
        "good_1.0.0.zip",
        &[(
            "good_1.0.0/info.json",
            &crate::testutil::info_json("good", "1.0.0", &[]),
        )],
    );

    let scan = scan_dir(dir.path());

    assert_eq!(scan.mods.len(), 1, "the readable mod must survive");
    assert!(!scan.warnings.is_empty());
}

#[test]
fn a_file_pretending_to_be_every_container_format_never_panics() {
    let dir = tempfile::tempdir().unwrap();
    let magics: [&[u8]; 5] = [
        b"BSA\0",
        b"BTDX",
        b"LSPK",
        &[0x34, 0x12, 0xAA, 0x55],
        &[0x00, 0x01, 0x00, 0x00],
    ];

    for (i, magic) in magics.iter().enumerate() {
        let mut bytes = magic.to_vec();
        // Absurd counts and offsets behind a plausible header.
        bytes.extend(std::iter::repeat_n(0xFFu8, 64));
        let name = dir.path().join(format!("hostile{i}.bsa"));
        std::fs::write(&name, &bytes).unwrap();

        // The contract: Err or Ok, but it comes back.
        let _ = crate::container::read(&name);
    }
}

#[test]
fn a_folder_of_hostile_archives_still_produces_a_report() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.bsa"), b"BSA\0\xff\xff\xff\xff").unwrap();
    std::fs::write(dir.path().join("b.pak"), b"LSPK\xff\xff\xff\xff").unwrap();
    std::fs::write(dir.path().join("c.vpk"), b"\x34\x12\xaa\x55\xff\xff").unwrap();
    crate::testutil::write_zip_mod(
        dir.path(),
        "good_1.0.0.zip",
        &[(
            "good_1.0.0/info.json",
            &crate::testutil::info_json("good", "1.0.0", &[]),
        )],
    );

    // Detection must still land, and the readable mod must still be reported.
    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_eq!(analysis.profile.name, "factorio");
    assert!(analysis.mods_scanned >= 1);
    assert!(!analysis.warnings.is_empty(), "the failures must be reported");
}

#[test]
fn a_malformed_plugin_never_takes_down_the_record_pass() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("BrokenMod");
    std::fs::create_dir_all(&broken).unwrap();
    // A plausible TES4 header followed by garbage lengths.
    let mut bytes = b"TES4".to_vec();
    bytes.extend(0xFFFF_FFFFu32.to_le_bytes());
    bytes.extend([0xAB; 40]);
    std::fs::write(broken.join("Broken.esp"), bytes).unwrap();

    let analysis = analyze::run(dir.path(), &Options::default()).unwrap();

    assert_eq!(analysis.profile.name, "creation-engine");
    assert_eq!(analysis.unreadable_plugins.len(), 1);
    // The mod is still counted; only its records are missing.
    assert_eq!(analysis.mods_scanned, 1);
}

#[test]
fn deeply_nested_json_is_rejected() {
    let json = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(value::load(json.as_bytes(), Format::Json).is_err());
}

/// The XML parser overflows the stack on a document this deep, and a stack
/// overflow aborts the process — so this must be refused before it is parsed.
/// If this test ever crashes rather than fails, the guard has been removed.
#[test]
fn deeply_nested_xml_is_refused_before_it_reaches_the_parser() {
    let xml = format!("{}{}", "<a>".repeat(50_000), "</a>".repeat(50_000));

    let err = value::load(xml.as_bytes(), Format::Xml).unwrap_err();

    assert!(format!("{err:#}").contains("past the limit"), "{err:#}");
}

#[test]
fn malformed_text_metadata_of_every_format_returns_an_error() {
    let cases: &[(&[u8], Format)] = &[
        (b"{ not json", Format::Json),
        (b"\xff\xfe\x00invalid utf8", Format::Json),
        (b"key = ", Format::Toml),
        (b"<unclosed>", Format::Xml),
        (b"", Format::Json),
    ];

    for (bytes, format) in cases {
        assert!(
            value::load(bytes, *format).is_err(),
            "expected an error for {format:?} on {bytes:?}"
        );
    }
}

#[test]
fn an_archive_entry_named_to_escape_the_folder_stays_a_plain_path() {
    let dir = tempfile::tempdir().unwrap();
    crate::testutil::write_zip_mod(
        dir.path(),
        "evil_1.0.0.zip",
        &[("../../../etc/passwd", "root:x:0:0")],
    );

    let scan = scan_dir(dir.path());

    // Nothing is ever extracted, so traversal cannot happen — but the path is
    // still recorded verbatim rather than resolved, and that is worth pinning.
    assert!(scan.mods[0]
        .files
        .iter()
        .any(|f| f.contains("etc/passwd")));
    assert!(!Path::new("/etc/passwd_modconflict_test").exists());
}

/// A deterministic mutation pass.
///
/// Proper fuzzing needs nightly and a library target this crate does not have,
/// so this is the stable-Rust version: seeded mutations of valid inputs, fed to
/// every parser, asserting only that control comes back. It is weaker than a
/// fuzzer at finding new cases and stronger at one thing — it runs on every
/// `cargo test`, so a regression cannot slip in unnoticed.
///
/// The seed is fixed: a failure here is reproducible, not a lottery.
mod mutation {
    use super::*;

    /// xorshift64*, chosen because it is five lines and does not need a crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as usize
            }
        }
    }

    fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
        let mut out = seed.to_vec();
        if out.is_empty() {
            return out;
        }
        match rng.below(4) {
            // Flip a byte.
            0 => {
                let i = rng.below(out.len());
                out[i] = rng.below(256) as u8;
            }
            // Truncate.
            1 => out.truncate(rng.below(out.len())),
            // Duplicate a slice, to grow nesting and counts.
            2 => {
                let start = rng.below(out.len());
                let end = start + rng.below(out.len() - start);
                let slice = out[start..=end.min(out.len() - 1)].to_vec();
                out.extend(slice);
            }
            // Insert a byte.
            _ => {
                let i = rng.below(out.len());
                out.insert(i, rng.below(256) as u8);
            }
        }
        out
    }

    const SEEDS: &[(&str, Format)] = &[
        (r#"{"name":"a","version":"1.0","dependencies":["b >= 1.0"]}"#, Format::Json),
        ("[[mods]]
modId = \"a\"
version = \"1.0\"
", Format::Toml),
        (
            r#"<modDesc descVersion="60"><version>1.0</version></modDesc>"#,
            Format::Xml,
        ),
    ];

    #[test]
    fn mutated_metadata_never_panics_and_always_returns() {
        let mut rng = Rng(0x5EED_1234_ABCD_0001);

        for (seed, format) in SEEDS {
            for _ in 0..2_000 {
                let bytes = mutate(seed.as_bytes(), &mut rng);
                // Every format is tried on every mutation: a JSON mutation fed
                // to the XML reader is exactly the kind of confusion a wrong
                // profile produces in the field.
                let _ = value::load(&bytes, *format);
                let _ = value::load(&bytes, Format::Json);
                let _ = value::load(&bytes, Format::Toml);
                let _ = value::load(&bytes, Format::Xml);
            }
        }
    }

    #[test]
    fn mutated_container_headers_never_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut rng = Rng(0x5EED_1234_ABCD_0002);
        let seeds: [&[u8]; 4] = [
            // Bethesda BSA, Fallout BA2, Larian LSPK, Valve VPK: a plausible
            // header each, written as bytes so no escape can be mangled.
            &[0x42, 0x53, 0x41, 0x00, 0x68, 0, 0, 0, 0x24, 0, 0, 0, 3, 0, 0, 0],
            &[0x42, 0x54, 0x44, 0x58, 1, 0, 0, 0, 0x47, 0x4E, 0x52, 0x4C, 2, 0, 0, 0],
            &[0x4C, 0x53, 0x50, 0x4B, 0x12, 0, 0, 0, 8, 0, 0, 0],
            &[0x34, 0x12, 0xAA, 0x55, 2, 0, 0, 0, 0x40, 0, 0, 0],
        ];

        for (i, seed) in seeds.iter().enumerate() {
            let path = dir.path().join(format!("fuzz{i}.bsa"));
            for _ in 0..250 {
                let bytes = mutate(seed, &mut rng);
                std::fs::write(&path, &bytes).unwrap();
                // Err is fine, Ok is fine, coming back at all is the assertion.
                let _ = crate::container::read(&path);
            }
        }
    }

    #[test]
    fn the_xml_depth_guard_never_panics_on_garbage() {
        let mut rng = Rng(0x5EED_1234_ABCD_0003);
        let seed = br#"<a><b attr=">"/><c/></b></a>"#;

        for _ in 0..5_000 {
            let bytes = mutate(seed, &mut rng);
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let _ = limits::max_xml_depth(text);
            }
        }
    }
}
