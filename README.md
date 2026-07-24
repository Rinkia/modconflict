# ModConflict

Scan a game mod folder and report conflicts **before** the game crashes.

Point it at your mod directory. It reads every archive, works out what each mod
claims to own and what it needs, and tells you what will break — missing
dependencies, wrong versions, duplicate ids, declared incompatibilities, files
that two mods both overwrite.

Read-only. It never writes to your mod folder.

```
$ modconflict "C:\Users\me\AppData\Roaming\Factorio\mods"
Factorio: scanned 4 mods
3 conflicts (3 critical)

[CRIT] alpha is incompatible with gamma
[CRIT] alpha needs base >=2.0.0
[CRIT] delta needs missing nonexistent-mod

Run with --tui for details.
```

`--tui` opens an interactive browser: conflict list on the left, the full
explanation and suggested fix on the right.

```
modconflict ~/.factorio/mods --tui
```

`↑↓`/`jk` move · `/` filter by mod or title · `c` clear filter · `f` cycle
minimum severity · `q` quit

## Install

```bash
cargo install --git https://github.com/Rinkia/modconflict
```

Or build it yourself:

```bash
git clone https://github.com/Rinkia/modconflict
cd modconflict
cargo build --release
```

## Usage

```
modconflict <PATH> [OPTIONS]

Arguments:
  <PATH>  Folder containing the mods

Options:
  -g, --game <GAME>  Which game the folder belongs to [possible values: factorio]
  -t, --tui          Browse the results in an interactive terminal UI
```

The game is detected automatically from the mod metadata; `--game` overrides it.

Exit codes make it usable as a pre-launch check: `0` clean, `1` conflicts
found, `2` error.

## What it detects

| Check | Severity | Meaning |
|-------|----------|---------|
| Missing dependency | critical | A required mod is not installed |
| Version mismatch | critical | The dependency is installed, but the wrong version |
| Duplicate id | critical | Two mods claim the same identifier |
| Declared incompatibility | critical | A mod says it cannot run alongside another installed mod |
| File overlap | warning | Two mods ship the same internal path — last one loaded wins |

File overlap is only a warning on purpose: compatibility patches overlap
deliberately, and a checker that cries wolf gets ignored.

## Supported games

- **Factorio** — reads `info.json`: mod id, version, and the full dependency
  syntax (`base >= 1.1.0`, `? optional`, `(?) hidden`, `! incompatible`,
  `~ no-load-order`).

Minecraft (Fabric/Forge), Skyrim, and Farming Simulator are the next targets.
The detector is already game-agnostic — it only reads a generic `ModEntry`, so
adding a game is one parser module plus one match arm, with no change to the
conflict logic.

## Architecture

```
scan.rs      walk the folder, open archives, inventory files + metadata bytes
games/       per-game parser: RawMod -> ModEntry   (the only game-aware code)
model.rs     ModEntry, Conflict, Severity          (the shared vocabulary)
conflict.rs  detection: pure function, no I/O      (the only logic that matters)
tui.rs       ratatui front end; all state in App, testable without a terminal
main.rs      CLI and text report
```

## Development

```bash
cargo test
cargo clippy --all-targets
```

Tests build throwaway zip archives in a temp directory, so the suite runs
anywhere, offline, with no game installed.

## Known limits

- Factorio prototype-name collisions live inside `data.lua` and would need a
  Lua parser. Only mod-id level checks today.
- Load order is not modelled yet, so "which mod wins" is not resolved for
  games where the order is configurable.

## License

MIT
