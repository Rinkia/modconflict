//! Game-specific parsing: turn a `RawMod` into a `ModEntry`.
//!
//! Adding a game means adding a module here plus a match arm — the detector
//! never changes.

pub mod factorio;

use anyhow::{bail, Result};

use crate::conflict::DetectOptions;
use crate::model::ModEntry;
use crate::scan::RawMod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Game {
    Factorio,
}

impl Game {
    pub fn parse_mods(self, raw: &[RawMod]) -> Vec<ModEntry> {
        match self {
            Game::Factorio => raw.iter().map(factorio::parse).collect(),
        }
    }

    pub fn detect_options(self) -> DetectOptions {
        match self {
            // Every Factorio mod lives under its own `__name__` namespace, so
            // two mods shipping `data.lua` is normal, not a conflict.
            Game::Factorio => DetectOptions {
                check_file_overlap: false,
            },
        }
    }

    /// Guess the game from what the mods look like.
    pub fn detect(raw: &[RawMod]) -> Result<Game> {
        if raw.iter().any(|m| m.metadata_named("info.json").is_some()) {
            return Ok(Game::Factorio);
        }
        bail!(
            "could not tell which game this folder is for (no info.json found). \
             Pass --game explicitly. Supported: factorio"
        )
    }
}

impl std::fmt::Display for Game {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Game::Factorio => f.write_str("Factorio"),
        }
    }
}
