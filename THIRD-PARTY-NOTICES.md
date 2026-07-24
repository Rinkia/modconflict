# Third-party notices

ModConflict's own source is MIT — see [LICENSE](LICENSE).

That is not the whole story for a **binary**, because a compiled binary contains
its dependencies. This file says exactly what that means, because "MIT" alone
would have been an incomplete claim.

## The one that matters: `esplugin` is GPL-3.0

Record-level conflict detection for Creation Engine games (Skyrim, Fallout,
Starfield) is built on [`esplugin`](https://crates.io/crates/esplugin), the
plugin parser behind LOOT. It is licensed **GPL-3.0**.

GPL-3.0 is copyleft: a binary that links it must be distributed under GPL-3.0.
MIT source code is compatible with that — the combination is simply governed by
the stronger licence.

So:

| What | Licence |
|------|---------|
| This repository's source code | MIT |
| A binary built with default features | **GPL-3.0**, because it contains `esplugin` |
| A binary built with `--no-default-features` | MIT and other permissive licences only |

The `records` feature exists for exactly this reason. It is on by default
because record-level detection is a headline feature and most people want it;
turning it off produces a working tool that simply does not compare plugin
records:

```bash
# Every feature. The resulting binary is GPL-3.0.
cargo build --release

# No esplugin, no record comparison. The resulting binary is permissive
# throughout. File overlaps, dependencies and every other game still work.
cargo build --release --no-default-features
```

If you redistribute a binary, this is your obligation to get right, not ours.

## Everything else is permissive

| Crate | Licence | Used for |
|-------|---------|----------|
| `anyhow` | MIT OR Apache-2.0 | error handling |
| `clap` | MIT OR Apache-2.0 | command line |
| `crossterm` | MIT | terminal control |
| `ratatui` | MIT | the TUI |
| `semver` | MIT OR Apache-2.0 | version requirements |
| `serde`, `serde_json` | MIT OR Apache-2.0 | JSON metadata |
| `toml` | MIT OR Apache-2.0 | TOML metadata and profiles |
| `roxmltree` | MIT OR Apache-2.0 | XML metadata |
| `walkdir` | Unlicense OR MIT | folder scanning |
| `zip` | MIT | `.zip` and `.jar` mods |
| `ba2` | 0BSD | Bethesda `.bsa` / `.ba2` archives |
| `vpk` | MIT | Source engine `.vpk` archives |
| `unpak` | MIT OR Apache-2.0 | Unreal `.pak` archives |
| `larian-formats` | Apache-2.0 | Baldur's Gate 3 `.pak` and `meta.lsx` |

Apache-2.0 and 0BSD are permissive and impose no copyleft obligation on the
combined work; Apache-2.0 asks that its notice be preserved, which is what this
file does.

## Keeping this accurate

Adding a dependency means checking its licence. A second copyleft dependency
would need the same treatment as `esplugin`: a feature gate and a row in the
table above. `cargo tree -f "{p} {l}"` prints the whole graph with licences.

Game metadata formats, file layouts and archive structures are facts about
those games, not copyrightable material, and no game files are redistributed
here — the test suite builds every fixture from scratch.
