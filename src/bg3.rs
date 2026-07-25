//! Baldur's Gate 3: metadata that lives inside a binary archive.
//!
//! BG3 mods ship as Larian `.pak` files with `Mods/<Name>/meta.lsx` inside.
//! The metadata *is* XML, but it is LSX — an object is spelled as a list of
//! `<attribute id="UUID" value="..."/>` elements, so reading it declaratively
//! would need path predicates, and a profile language with predicates is a
//! query language wearing a disguise.
//!
//! `larian-formats` already parses it, so this module is the seam rather than
//! the parser: a profile names a code-backed metadata reader, and the reader
//! answers with the same id / version / dependencies any text profile produces.

use std::path::Path;

use anyhow::{Context, Result};
use larian_formats::{lspk::Lspk, UnpackedVersion};

use crate::model::{Dep, DepKind};

/// What a code-backed reader answers with — the same three things a text
/// profile pulls out of a manifest.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub id: Option<String>,
    pub version: Option<String>,
    pub requires: Vec<Dep>,
}

pub fn read(path: &Path) -> Result<Extracted> {
    let pak = Lspk::from_file(path).with_context(|| format!("{}: not a Larian pak", path.display()))?;
    let meta = pak
        .deserialize_meta_lsx()
        .with_context(|| format!("{}: no readable meta.lsx", path.display()))?;

    // Mods reference each other by UUID, so that is the id the dependency
    // check has to match on. The human name is not unique and not used.
    let info = meta.module_info;
    Ok(Extracted {
        id: non_empty(info.uuid),
        version: version_string(info.version),
        requires: meta
            .dependencies
            .into_iter()
            .filter_map(|dep| {
                Some(Dep {
                    name: non_empty(dep.uuid)?,
                    // A dependency records the version it was built against,
                    // not a floor, so treating it as a requirement would fail
                    // every legitimately updated mod.
                    req: None,
                    kind: DepKind::Required,
                    syntax: Default::default(),
                })
            })
            .collect(),
    })
}

/// BG3 packs `major.minor.revision.build` into one 64-bit field.
fn version_string(packed: i64) -> Option<String> {
    if packed <= 0 {
        return None;
    }
    Some(UnpackedVersion::from_packed(packed as u64).to_string())
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{bg3_meta_lsx as meta_lsx, write_bg3_pak as write_pak};

    #[test]
    fn reads_the_uuid_and_version_out_of_a_pak() {
        let dir = tempfile::tempdir().unwrap();
        // 1.0.0.0 packed.
        let packed = UnpackedVersion {
            major: 1,
            minor: 0,
            rev: 0,
            build: 0,
        }
        .to_packed() as i64;
        let path = write_pak(
            dir.path(),
            "CoolMod",
            &meta_lsx("11111111-1111-1111-1111-111111111111", "CoolMod", packed, &[]),
        );

        let extracted = read(&path).unwrap();

        assert_eq!(
            extracted.id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(extracted.version.as_deref(), Some("1.0.0.0"));
        assert!(extracted.requires.is_empty());
    }

    #[test]
    fn dependencies_are_recorded_by_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pak(
            dir.path(),
            "PatchMod",
            &meta_lsx(
                "22222222-2222-2222-2222-222222222222",
                "PatchMod",
                36028797018963968,
                &[("33333333-3333-3333-3333-333333333333", "BaseMod")],
            ),
        );

        let extracted = read(&path).unwrap();

        assert_eq!(extracted.requires.len(), 1);
        assert_eq!(
            extracted.requires[0].name,
            "33333333-3333-3333-3333-333333333333"
        );
        assert_eq!(extracted.requires[0].kind, DepKind::Required);
        // The recorded version is what the mod was built against, not a floor.
        assert_eq!(extracted.requires[0].req, None);
    }

    #[test]
    fn a_file_that_is_not_a_pak_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.pak");
        std::fs::write(&path, b"definitely not a pak").unwrap();

        assert!(read(&path).is_err());
    }
}
