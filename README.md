# ModConflict

Scan a game mod folder and report conflicts **before** the game crashes.

Point it at your mod directory. It reads every archive, works out what each mod
claims to own and what it needs, and tells you what will break — missing
dependencies, wrong versions, duplicate ids, declared incompatibilities, files
that two mods both overwrite.

Read-only. It never writes to your mod folder.

```
$ modconflict "C:\Users\me\AppData\Roaming\Factorio\mods"
Factorio: scanned 3 mods (1 disabled, skipped)
2 conflicts (2 critical)

[CRIT] alpha is incompatible with gamma
[CRIT] alpha needs base >=2.0.0

Run with --tui for details.
```

`--tui` opens an interactive browser: conflict list on the left, the full
explanation and suggested fix on the right.

```
modconflict ~/.factorio/mods --tui
```

`↑↓`/`jk` move · `/` filter by mod or title · `c` clear filter · `f` cycle
minimum severity · `q` quit

## It is not tied to one game

Almost every modern game ships mod metadata as a JSON, TOML or XML file inside
the mod archive. They differ in field names and dependency syntax — nothing
more. So a game is **not** code here, it is a data file:

```toml
# profiles/factorio.toml
name = "factorio"
display_name = "Factorio"
metadata_file = "info.json"
format = "json"
id_field = "name"
version_field = "version"
check_file_overlap = false      # each mod has its own namespace

[[dependencies]]
field = "dependencies"
syntax = "prefixed-strings"     # "base >= 1.1.0", "? optional", "! breaks"

[load_order]
file = "mod-list.json"
format = "json"
path = "mods"
name_field = "name"
enabled_field = "enabled"
```

Drop a `.toml` file in a directory, pass `--profiles <DIR>`, and the game is
supported. No fork, no recompile, no pull request. A user profile overrides a
built-in of the same name, so a stale built-in can be fixed locally without
waiting for a release.

The conflict detector never sees any of this: it reads a generic model, so
every game gets every check for free, and a new game adds zero risk to the
existing ones.

### Profile reference

| Key | Meaning |
|-----|---------|
| `metadata_file` | Filename to look for inside each mod, matched at any depth |
| `format` | `json`, `toml` or `xml` |
| `root` | Path prefix into the document, e.g. `modDesc` for XML |
| `id_field` | Path to the mod id. Omit it and the filename is used |
| `version_field` | Path to the version |
| `provides_field` | Extra ids the mod claims to satisfy |
| `check_file_overlap` | `false` when mods are namespaced and overlap is normal |
| `[[dependencies]]` | One or more dependency collections, see below |
| `[load_order]` | Where the game records load order, if it has one |

Field paths are dotted (`mods.modId`). A `*` segment fans out over a map or
list, which is how Forge's `dependencies.<your-own-mod-id>` tables are read
without knowing the key in advance.

Three dependency shapes cover what games actually use:

- `prefixed-strings` — a list like `["base >= 1.1.0", "? optional", "! breaks"]`
  (Factorio, and plain names for simpler games)
- `map` — `{"fabricloader": ">=0.14.0"}` (Fabric)
- `tables` — a list of tables with `name_field` / `version_field` /
  `required_field` (Forge)

Two load-order shapes cover the rest:

- `lines` — one entry per line, with an optional `enabled_prefix` (Skyrim's
  `plugins.txt` marks enabled plugins with `*`)
- `json` / `toml` — a list of entries with a name field and an enabled field
  (Factorio's `mod-list.json`)

The honest limit: profiles read **text** metadata. A game that hides its mod
data in a proprietary binary format — Skyrim's `.esp` records, for instance —
needs real code, and no amount of configuration substitutes for it.

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
  -g, --game <GAME>         Which game the folder belongs to
  -t, --tui                 Browse the results in an interactive terminal UI
  -j, --json                Print the report as JSON
  -l, --load-order <FILE>   Load order file, overriding the profile's default
      --profiles <DIR>      Directory of extra game profiles (.toml)
      --list-games          List the games this build knows about
```

The game is detected automatically from the mod metadata; `--game` overrides
it.

Exit codes make it usable as a pre-launch check: `0` clean, `1` conflicts
found, `2` error.

### JSON output

`--json` prints a stable envelope for scripts and CI. Every conflict carries a
`kind`, a `severity`, the human `title` and `detail`, plus its own typed
fields:

```json
{
  "game": "factorio",
  "mods_scanned": 3,
  "mods_disabled": 1,
  "load_order_known": true,
  "conflict_count": 2,
  "critical_count": 2,
  "conflicts": [
    {
      "severity": "critical",
      "title": "alpha needs base >=2.0.0",
      "detail": "alpha requires \"base\" >=2.0.0, but version 1.1.0 is installed.\n\nUpdate \"base\", or downgrade alpha.",
      "kind": "version_mismatch",
      "mod_id": "alpha",
      "dep": { "name": "base", "req": ">=2.0.0", "kind": "required" },
      "found": "1.1.0"
    }
  ]
}
```

### Load order

When the profile knows where the game records load order, ModConflict reads it
and uses it for two things:

- **Disabled mods are skipped.** A mod the player switched off cannot conflict
  with anything, and reporting it is noise.
- **File overlaps name a winner.** Without a load order, "two mods ship this
  file" is all anyone can say. With one, the report says which mod actually
  wins and which copies the game silently ignores.

`--load-order <FILE>` points at a specific file when it lives somewhere
unusual.

## What it detects

| Check | Severity | Meaning |
|-------|----------|---------|
| Missing dependency | critical | A required mod is not installed |
| Version mismatch | critical | The dependency is installed, but the wrong version |
| Duplicate id | critical | Two mods claim the same identifier |
| Declared incompatibility | critical | A mod says it cannot run alongside another installed mod |
| File overlap | warning | Two mods ship the same internal path — the loser is silently ignored |

File overlap is only a warning on purpose: compatibility patches overlap
deliberately, and a checker that cries wolf gets ignored.

## Games out of the box

| Profile | Metadata | Notes |
|---------|----------|-------|
| `factorio` | `info.json` | Full dependency syntax, `mod-list.json` load order |
| `minecraft-fabric` | `fabric.mod.json` | `depends` / `recommends` / `breaks` / `conflicts`, `provides` |
| `minecraft-forge` | `META-INF/mods.toml` | Forge and NeoForge dependency tables |
| `farming-simulator` | `modDesc.xml` | Mod id comes from the zip filename |

`--list-games` prints what your build knows, including your own profiles.

## Architecture

```
scan.rs       walk the folder, open archives, inventory files + metadata bytes
value.rs      JSON/TOML/XML collapsed into one document tree with dotted paths
profile.rs    the game profile schema, the built-ins, and autodetection
parse.rs      Profile + RawMod -> ModEntry     (data-driven, no per-game code)
model.rs      ModEntry, Conflict, Severity     (the shared vocabulary)
conflict.rs   detection: pure function, no I/O (the only logic that matters)
loadorder.rs  who is enabled, and who wins an overlap
report.rs     text and JSON output
tui.rs        ratatui front end; all state in App, testable without a terminal
main.rs       CLI wiring
```

## Development

```bash
cargo test
cargo clippy --all-targets
```

Tests build throwaway zip archives in a temp directory, so the suite runs
anywhere, offline, with no game installed.

## Known limits

- Binary mod formats (Skyrim's `.esp`/`.esm`) cannot be described by a profile
  and are not supported yet.
- Version comparison is semver. Requirements in another dialect — Forge's
  Maven ranges, for instance — are treated as satisfied rather than guessed at,
  because a false alarm is worse than a miss here.
- Factorio prototype-name collisions live inside `data.lua` and would need a
  Lua parser. Only mod-id level checks today.

## License

MIT
