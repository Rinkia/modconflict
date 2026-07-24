//! Caps on untrusted input.
//!
//! Everything this tool opens was downloaded from the internet by someone who
//! wanted a nicer sword. A mod archive can claim ten million entries, or a
//! kilobyte that expands to a gigabyte, and until these limits existed the
//! answer to both was to try it and run out of memory.
//!
//! The rule throughout: exceeding a limit is a **warning and a truncation**,
//! never a crash and never a silent success. A scan that hits a cap still
//! reports everything it did manage to read, and says what it skipped.

/// Entries read from one zip or jar. A folder of normal mods is thousands;
/// a hundred thousand is already absurd.
pub const MAX_ARCHIVE_ENTRIES: usize = 200_000;

/// Entries read from one binary container. Bethesda archives legitimately hold
/// tens of thousands of textures, so this is deliberately generous.
pub const MAX_CONTAINER_ENTRIES: usize = 500_000;

/// A metadata file this large is not a manifest. Reading it into memory is the
/// only place the scanner holds file *contents* rather than paths.
pub const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;

/// How deep a metadata document may nest.
///
/// JSON and TOML parsers cap their own recursion. The XML one does not: a
/// 50,000-deep document overflows the stack inside `roxmltree::Document::parse`
/// itself, before any of this crate's code runs. A stack overflow aborts the
/// process — `catch_unwind` cannot save it — so the depth has to be checked
/// *before* the parser is handed the text. Real manifests nest a handful of
/// levels; this leaves an order of magnitude of headroom.
pub const MAX_DOCUMENT_DEPTH: usize = 100;

/// Nodes one XML document may contain, to bound memory on a wide document the
/// way `MAX_DOCUMENT_DEPTH` bounds the stack on a deep one.
pub const MAX_XML_NODES: u32 = 1_000_000;

/// An upper bound on how deeply `text` nests XML elements.
///
/// Deliberately a scanner and not a parser: it over-counts on `>` inside
/// attribute values and on angle brackets inside comments. Over-counting only
/// makes it stricter, which is the safe direction for a guard whose whole job
/// is to run before a parser that would crash the process.
pub fn max_xml_depth(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut depth: usize = 0;
    let mut deepest: usize = 0;

    for (i, window) in bytes.iter().enumerate() {
        if *window != b'<' {
            continue;
        }
        match bytes.get(i + 1) {
            Some(b'/') => depth = depth.saturating_sub(1),
            // Comment, DOCTYPE or processing instruction: no nesting of ours.
            Some(b'!') | Some(b'?') => {}
            None => {}
            _ => {
                let closes_at = bytes[i..].iter().position(|&b| b == b'>').map(|p| i + p);
                let self_closing = closes_at
                    .map(|end| end > i && bytes[end - 1] == b'/')
                    .unwrap_or(false);
                if !self_closing {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
            }
        }
    }
    deepest
}

/// Uncompressed-to-compressed ratio above which a metadata entry is treated as
/// a decompression bomb and skipped. Real JSON compresses well — 20x is
/// ordinary, 200x is an attack.
pub const MAX_DECOMPRESSION_RATIO: u64 = 200;

/// Whether a zip entry's declared sizes look like a decompression bomb.
///
/// Zero compressed size with a large declared size is the classic shape, so it
/// is treated as suspicious rather than as a division to avoid.
pub fn is_decompression_bomb(uncompressed: u64, compressed: u64) -> bool {
    if uncompressed <= MAX_METADATA_BYTES {
        return false;
    }
    match compressed {
        0 => true,
        c => uncompressed / c > MAX_DECOMPRESSION_RATIO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_ordinary_nesting() {
        assert_eq!(max_xml_depth("<a><b><c>x</c></b></a>"), 3);
        assert_eq!(max_xml_depth("<a/>"), 0);
        assert_eq!(max_xml_depth("<a><b/></a>"), 1);
    }

    #[test]
    fn siblings_do_not_add_depth() {
        assert_eq!(max_xml_depth("<a><b>1</b><b>2</b><b>3</b></a>"), 2);
    }

    #[test]
    fn a_nesting_bomb_is_measured_as_deep() {
        let xml = format!("{}{}", "<a>".repeat(5_000), "</a>".repeat(5_000));
        assert!(max_xml_depth(&xml) > MAX_DOCUMENT_DEPTH);
    }

    #[test]
    fn a_real_manifest_stays_far_below_the_limit() {
        let xml = r#"<?xml version="1.0"?>
            <modDesc descVersion="60">
              <version>1.0.0.0</version>
              <title><en>Cool</en></title>
              <dependencies><dependency>Other</dependency></dependencies>
            </modDesc>"#;
        assert!(max_xml_depth(xml) < 10, "{}", max_xml_depth(xml));
    }

    #[test]
    fn ordinary_compression_is_not_a_bomb() {
        // A 1 MB manifest compressing to 40 KB is 25x, and normal.
        assert!(!is_decompression_bomb(1_000_000, 40_000));
    }

    #[test]
    fn a_small_entry_is_never_a_bomb_however_it_compresses() {
        // Below the size cap there is nothing to protect against: the read is
        // bounded regardless of the ratio.
        assert!(!is_decompression_bomb(1000, 1));
    }

    #[test]
    fn a_large_entry_with_an_extreme_ratio_is_a_bomb() {
        assert!(is_decompression_bomb(1024 * 1024 * 1024, 1024));
    }

    #[test]
    fn a_large_entry_claiming_zero_compressed_size_is_a_bomb() {
        assert!(is_decompression_bomb(u64::MAX, 0));
    }
}
