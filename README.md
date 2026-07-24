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
| `detect_extensions` | Identifies the game when it has no metadata file at all |
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
- `tables` — a list of tables with `name_field` / `version_field` and either
  `required_field` (`false` means optional, Forge) or `optional_field`
  (`true` means optional, Bannerlord)

`kind` on a source sets what a plain entry means, so the same
`prefixed-strings` syntax reads as a dependency list in one place and as
RimWorld's `incompatibleWith` in another.

`version_prefix` turns a bare version into a requirement. SMAPI's
`MinimumVersion: "1.2.0"` is a floor; read literally semver takes it for
`^1.2.0` and rejects every later major version — a false alarm on well-formed
mods.

Two load-order shapes cover the rest:

- `lines` — one entry per line, with an optional `enabled_prefix` (Skyrim's
  `plugins.txt` marks enabled plugins with `*`)
- `json` / `toml` — a list of entries with a name field and an enabled field
  (Factorio's `mod-list.json`)

Profiles read **text** metadata. Games that hide everything in binary archives
are handled by a second layer — see below.

## Binary archives

Some games ship no text metadata at all. A Skyrim mod is loose files, `.esp`
plugins and `.bsa` archives; an Unreal game's mods are `.pak` files. A profile
cannot describe those, and a byte-level description language in TOML would just
be a parser written in the wrong language.

So the second layer is code — but the *general* part is the contract, not the
parsing:

> a container reader takes a file and returns the paths inside it.

That one answer is enough to put binary archives through every check the
detector already does. A texture packed inside a `.bsa` and a loose copy of the
same texture in another mod now collide in the report exactly the way they
collide in the game — something a filename-only scan cannot see at all.

The parsers are not ours. They are maintained crates that already track each
format's version drift, which is the part that actually rots:

| Format | Games | Crate |
|--------|-------|-------|
| `.bsa` / `.ba2` | Morrowind through Starfield | [`ba2`](https://crates.io/crates/ba2) |
| `.vpk` | Source engine | [`vpk`](https://crates.io/crates/vpk) |
| `.pak` | Unreal Engine 4 and 5 | [`unpak`](https://crates.io/crates/unpak) |
| `.pak` (LSPK) | Baldur's Gate 3 and other Larian games | [`larian-formats`](https://crates.io/crates/larian-formats) |

Archives are identified by magic bytes, not by extension, because renamed
extensions are common — Baldur's Gate 3 and Unreal both use `.pak` for two
entirely unrelated formats, and only the header tells them apart. Adding a
format is one entry in a table: a sniff function and a read function.

An archive lying directly in the mods folder is itself a mod, which is how BG3
and most Unreal games ship.

### When the metadata is inside the archive too

Some games lock the manifest in there as well. A BG3 mod keeps
`Mods/<Name>/meta.lsx` inside its `.pak` — XML, but LSX spells an object as a
list of `<attribute id="UUID" value="..."/>` elements, so reaching a field
declaratively would need path predicates. A profile language with predicates is
a query language wearing a disguise, so instead a profile can name a
**code-backed metadata reader**:

```toml
metadata_reader = "bg3-pak"
```

The reader answers with the same id, version and dependencies a text profile
produces, and everything downstream is unchanged.

## The record level

For Creation Engine games the file list is still not the real story. The
conflict Skyrim and Fallout players actually hit is two plugins editing the
**same record** — the same NPC, the same weapon, the same cell. Nothing in the
filenames reveals it.

```
$ modconflict "D:\MO2\Skyrim\mods"
Skyrim / Fallout / Starfield: scanned 4 mods
read 4 plugins at record level
2 conflicts (1 critical)

[CRIT] PatchMod needs missing MissingBigMod.esp
[WARN] BetterWeapons.esp and WeaponRebalance.esp both edit 2 records
```

[`esplugin`](https://crates.io/crates/esplugin) — the library behind LOOT —
does the parsing. What ModConflict adds is the translation into the shared
model, so record findings land next to every other kind of conflict:

- **Record overlap** is a *warning*, not an error. Overlapping is how
  compatibility patches work. The report says how many records two plugins
  share, and that the later one wins them all.
- **Masters become dependencies.** A plugin's masters are checked like any
  other dependency, so a patch whose base mod is not installed is a missing
  dependency with a clear message.
- **The game's own masters are not dependencies.** `Skyrim.esm` lives in the
  game folder, never in a mod folder, so requiring it is not a problem. The
  `base_ids` list in the profile says which masters those are — extend it in a
  user profile for whichever game you are scanning.
- **Plugin filenames become symbols.** Two mods installing the same `.esp`
  filename is a genuine clash: only one file survives on disk.

FormIDs are stored relative to each plugin's own master list, so they mean
nothing across plugins until resolved against it — ModConflict resolves before
comparing, because skipping that step compares two different numbering schemes
and calls the result an overlap.

Parsing every plugin is the slow part of a large load order. `--no-records`
skips it.

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
      --no-records          Skip the record-level pass (the slow part)
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
| Record overlap | warning | Two plugins edit the same records — the later one wins them all |

File overlap is only a warning on purpose: compatibility patches overlap
deliberately, and a checker that cries wolf gets ignored.

## Games out of the box

| Profile | Metadata | Notes |
|---------|----------|-------|
| `factorio` | `info.json` | Full dependency syntax, `mod-list.json` load order |
| `minecraft-fabric` | `fabric.mod.json` | `depends` / `recommends` / `breaks` / `conflicts`, `provides` |
| `minecraft-forge` | `META-INF/mods.toml` | Forge and NeoForge dependency tables |
| `farming-simulator` | `modDesc.xml` | Mod id comes from the zip filename |
| `stardew-valley` | `manifest.json` | SMAPI: `Dependencies`, `ContentPackFor` |
| `rimworld` | `About/About.xml` | `modDependencies`, `incompatibleWith` |
| `bannerlord` | `SubModule.xml` | `DependedModules`, attribute-carried values |
| `baldurs-gate-3` | `meta.lsx` inside the `.pak` | Mods identified by UUID |
| `creation-engine` | none — `.esp`/`.bsa` | Skyrim, Fallout, Starfield; archives expanded, records compared |

`--list-games` prints what your build knows, including your own profiles.

## Architecture

```
scan.rs       walk the folder, open archives, inventory files + metadata bytes
container.rs  binary archives (.bsa/.ba2/.vpk/.pak) -> the paths inside them
bg3.rs        code-backed metadata reader for Larian paks
records.rs    Creation Engine plugins -> record overlaps, masters, plugin names
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

### Every profile must prove itself

A profile is a claim about a game's metadata format, and a wrong claim fails
*silently*: the mod parses, the id is wrong, and the report is confidently
useless. So each profile ships a fixture, and the test suite refuses a profile
that has none:

```
profiles/fixtures/<profile>/
  input/<metadata file>     a sample, as the game's own docs describe it
  expected.json             the exact id, version, provides and requires
```

`expected.json` carries a `source_of_truth` field naming where the format claim
comes from, so a wrong fixture can be traced rather than argued about.

What this proves is that the profile matches the format **as documented**. It
does not prove the documentation matches the mods people actually publish —
only a corpus of real mods does that, and that is the next step.

## Known limits

- **No real mod has been through this tool yet.** Every test uses fixtures
  built from format documentation. The parsing libraries are tested upstream,
  but the integration around them — path heuristics, id fallbacks, game
  detection — is calibrated on invented examples. Treat the profiles as
  informed claims until a corpus of real mods says otherwise.
- Bannerlord versions are written `v1.0.0`, which semver cannot read, so its
  version requirements come out unverified rather than wrong.

- Record comparison is pairwise and parses every plugin whole, so a very large
  load order costs time and memory. `--no-records` turns it off.
- Plugins are read from disk, so a mod still packed as a `.zip` is not analysed
  at the record level. Creation Engine mods are installed as folders.
- The Creation Engine profile deliberately has no load order. `plugins.txt`
  lists plugin names while a mod id here is the mod folder name; mapping one to
  the other is the mod manager's job, and guessing would name the wrong winner
  with total confidence.
- Archives nested inside a `.zip` are not expanded — only archives sitting in a
  mod folder are.
- Version comparison is semver. Requirements in another dialect — Forge's
  Maven ranges, for instance — are treated as satisfied rather than guessed at,
  because a false alarm is worse than a miss here.
- Factorio prototype-name collisions live inside `data.lua` and would need a
  Lua parser. Only mod-id level checks today.

## License

MIT
