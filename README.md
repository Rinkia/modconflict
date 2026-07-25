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
| `version_syntax` | Dialect of the version requirements: `semver` (default) or `maven` |
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
  -m, --manager <MANAGER>   Mod manager governing the folder [mo2, none]
      --no-records          Skip the record-level pass (the slow part)
      --no-hash             Skip hashing overlapping files
      --list-games          List the games this build knows about
```

The game is detected automatically from the mod metadata; `--game` overrides
it.

Exit codes make it usable as a pre-launch check: `0` nothing actionable, `1`
something worth looking at, `2` error. `INFO` findings do not fail the check.

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

## Identical copies are not conflicts

Two mods shipping the same path is a warning. Plenty of them should not be: mod
packs bundle the same library, authors reupload an unchanged asset, a patch
ships a file it never touched. Whichever copy the game loads, it gets the same
file — and a checker that cries wolf gets ignored.

So the overlapping files, and only those, are hashed:

```
$ modconflict ~/mods
Minecraft (Fabric): scanned 3 mods
3 conflicts (0 critical)

[INFO] 3 mods ship an identical assets/shared.png
[INFO] CoolMod is a duplicate of OtherMod (1 identical file)
[INFO] CoolModCopy is a duplicate of CoolMod (1 identical file)
```

```
$ modconflict ~/mods --no-hash
[WARN] 3 mods ship assets/shared.png
```

`INFO` findings **do not fail the exit code**: the first run above exits `0`,
the second exits `1` over a non-problem. Exiting non-zero over things that
change nothing in the game is how people learn to ignore an exit code.

A mod whose every file is an identical copy of another's is reported as a
duplicate install rather than a pile of overlaps — the manifest is excluded from
that comparison, since it necessarily differs by carrying the id.

The scan itself still never reads file contents. A folder with no overlaps costs
no reads at all, and `--no-hash` turns even that off. Files inside a binary
archive cannot be compared this way and are left honestly unresolved.

## Mod managers

The Creation Engine profile ships without a load order **on purpose**:
`plugins.txt` lists *plugin* names while a mod id here is the mod folder name,
and guessing the mapping would name the wrong winner with total confidence.

A mod manager knows both. Point ModConflict at a Mod Organizer 2 instance's
`mods/` folder and it reads what MO2 wrote:

```
$ modconflict "D:\MO2\Skyrim\mods"
Skyrim / Fallout / Starfield: scanned 2 mods
read 2 plugins at record level
load order from Mod Organizer 2, profile "Default"
2 conflicts (0 critical)

[WARN] 2 mods ship textures/iron.dds (WeaponRebalance wins)
[WARN] BetterWeapons.esp and WeaponRebalance.esp both edit 2 records (WeaponRebalance.esp wins)
```

Without it, the same two clashes are found but no winner is invented:

```
$ modconflict "D:\MO2\Skyrim\mods" --manager none
[WARN] 2 mods ship textures/iron.dds
[WARN] BetterWeapons.esp and WeaponRebalance.esp both edit 2 records
```

The plugin-to-mod mapping is never read from anywhere: the scan already knows
which mod ships which `.esp`. Only the two orderings were missing — which mod
overwrites which (`modlist.txt`) and which plugin loads after which
(`loadorder.txt`, else `plugins.txt`) — and both are plain text.

`ModOrganizer.ini` names the active profile; failing that, the only profile
present is used.

**Detection** is automatic: an MO2 instance keeps `mods/` and `profiles/` side
by side. `--manager mo2` demands one and errors if it is absent; `--manager
none` ignores one that is there. An explicit `--load-order` still wins over
both.

Read-only, always. The manager's own files are never written.

**Vortex is not supported.** It keeps its state in a LevelDB rather than text
files, and reading it would mean reverse-engineering an undocumented internal
format that changes between releases. Saying so is more useful than guessing
wrong about which mod wins.

## Version requirements it can and cannot read

`semver` reads `>=1.1.0`, but Forge writes `[40,)` and Fabric writes `1.20.x`.
Feeding those to `semver` fails, and the old behaviour was to treat a failed
parse as *satisfied* — the right call, since a false alarm is worse than a miss,
but a silent one. Nobody could tell a real pass from a requirement the tool
simply could not read.

So a check now has three outcomes, not two:

- **Satisfied** / **Violated** — the version was compared.
- **Unverified** — the requirement or the version could not be read, so no
  claim is made. Never a false alarm, but no longer silent: the count is
  reported.

```
$ modconflict ~/mods --profiles ./semver-forge
note: 1 version requirement could not be checked (unreadable dialect) — treated as satisfied
no conflicts found
```

A profile declares its dialect with `version_syntax`; the default is `semver`,
which also understands `1.20.x` / `1.20.*` globs. `maven` reads Forge and
NeoForge ranges — `[40,)`, `[1.19,1.20)`, `(,2.0]`, comma-joined OR-sets, and a
bare `1.0` as the soft recommendation Maven means by it. With the Forge profile
set to `maven`, a mod needing `[40,)` against an installed `36` is now a real
critical, where before it passed unnoticed.

The JSON report carries the same figure as `unverified_requirements`.

## What it detects

| Check | Severity | Meaning |
|-------|----------|---------|
| Missing dependency | critical | A required mod is not installed |
| Version mismatch | critical | The dependency is installed, but the wrong version |
| Duplicate id | critical | Two mods claim the same identifier |
| Declared incompatibility | critical | A mod says it cannot run alongside another installed mod |
| File overlap | warning | Two mods ship the same internal path — the loser is silently ignored |
| Record overlap | warning | Two plugins edit the same records — the later one wins them all |
| Identical overlap | info | The clashing copies are byte-identical, so nothing is at stake |
| Redundant mod | info | Every file it ships is an identical copy of another mod's |

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
analyze.rs    the whole pipeline in one call, shared by the CLI and the tests
scan.rs       walk the folder, open archives, inventory files + metadata bytes
container.rs  binary archives (.bsa/.ba2/.vpk/.pak) -> the paths inside them
bg3.rs        code-backed metadata reader for Larian paks
records.rs    Creation Engine plugins -> record overlaps, masters, plugin names
value.rs      JSON/TOML/XML collapsed into one document tree with dotted paths
profile.rs    the game profile schema, the built-ins, and autodetection
parse.rs      Profile + RawMod -> ModEntry     (data-driven, no per-game code)
model.rs      ModEntry, Conflict, Severity     (the shared vocabulary)
conflict.rs   detection: pure function, no I/O (the only logic that matters)
hash.rs       are the clashing copies the same bytes, or different ones?
versionreq.rs is a requirement met? satisfied / violated / unverifiable
loadorder.rs  who is enabled, and who wins an overlap
manager.rs    Mod Organizer 2: mod priority and plugin order, read-only
report.rs     text and JSON output
tui.rs        ratatui front end; all state in App, testable without a terminal
main.rs       CLI wiring
```

## Development

```bash
cargo test
cargo clippy --all-targets

# The permissive-only build must keep compiling too.
cargo build --no-default-features
```

Tests build throwaway zip archives in a temp directory, so the suite runs
anywhere, offline, with no game installed.

### Is it actually reading your mods?

Every report says how much of the folder the profile understood. A profile that
is subtly wrong does not crash: it parses nothing, falls back to filenames, and
cheerfully reports a clean folder. So the tool says when that happens.

```
$ modconflict ~/factorio/mods --game rimworld
RimWorld: scanned 4 mods
warning: read metadata for only 0 of 4 mods (0%) — the rest fall back to their filename.
         If that is most of them, this is the wrong game profile.
no conflicts found
```

"no conflicts found" on its own would have been a dangerous lie. The JSON
report carries the same number as `mods_with_metadata`.

### Validating against real mods

Fixtures prove a profile matches the format **as documented**. Whether the
documentation matches what modders actually publish is a different question,
and only real mods answer it. Real mods cannot live in this repository — other
people's work, other people's licences, and large — so the corpus lives on your
machine and the harness is opt-in.

Point `MODCONFLICT_CORPUS` at a directory holding a `corpus.toml`:

```toml
[[entry]]
path = "factorio"              # relative to the corpus dir, or absolute
game = "factorio"              # the profile detection must land on
min_mods = 20
min_metadata_coverage = 0.95   # share of mods whose metadata must be read

[[entry]]
path = "/home/me/MO2/Skyrim/mods"
game = "creation-engine"
min_metadata_coverage = 0.0    # this game has no text metadata to read
max_unreadable_plugins = 2
max_seconds = 120.0
```

```bash
MODCONFLICT_CORPUS=/path/to/corpus cargo test --ignored corpus
```

Every entry runs even after one fails, so the output is a full picture rather
than the first thing that broke:

```
ok    factorio: factorio [43 mods, 100% understood, 7 conflicts, 0.4s]
FAIL  stardew: stardew-valley [61 mods, 62% understood, 3 conflicts, 0.6s]
    - understood the metadata of only 62% of mods (38 of 61), expected 95%
```

The assertions are about the health of the **tool**, never the cleanliness of
the mods: a real mod folder is expected to have conflicts, and a harness that
failed on them would be useless.

Building a corpus means pointing it at mod folders you already have — a
Factorio `mods/`, a Mod Organizer `mods/` tree, a Stardew `Mods/`. Coverage
below 100% on a folder you trust is a bug report waiting to be written.

### Frozen reports

`tests/snapshots/` holds the exact text and JSON a few known folders produce, so
a change in wording, ordering or counts shows up as a diff instead of shipping
unnoticed. After a deliberate change:

```bash
UPDATE_SNAPSHOTS=1 cargo test snapshot
```

### Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers the setup, the one invariant (this
tool reads, it never writes), and what a new game profile needs.

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

## Hostile input

Every file this tool opens was downloaded from the internet by someone who
wanted a nicer sword. A mod archive can claim ten million entries, or a kilobyte
that expands to a gigabyte, or nest ten thousand levels deep.

The rule throughout: exceeding a limit is a **warning and a truncation**, never
a crash and never a silent success. A scan that hits one still reports
everything it did manage to read, and says what it skipped.

```
$ modconflict ~/mods
Factorio: scanned 1 mod
warning: skipping a.bsa: unexpected end of file
warning: skipping b.pak: this tool does not support version number 4294967295
warning: skipping broken.zip: invalid Zip archive: Could not find EOCD
no conflicts found
```

Warnings are collected rather than printed to stderr, so `--json` consumers see
them too, in a `warnings` array.

| Limit | Why |
|-------|-----|
| 200,000 entries per zip | An index claiming more is not a mod |
| 500,000 entries per binary archive | Generous: Bethesda archives really do hold tens of thousands of textures |
| 8 MB per metadata file | The only place the scanner holds file *contents* rather than paths |
| 200:1 decompression ratio | 20x is ordinary for JSON, 200x is an attack |
| 100 levels of document nesting | Real manifests nest a handful |

Third-party parsers run behind a panic boundary. They are good crates, but they
are parsing hostile binary input, and an index-out-of-bounds deep inside one
would otherwise take down a scan of two hundred perfectly readable mods. A
panic costs exactly one archive, and says so.

The nesting limit is checked **before** the XML parser is handed the text, not
after: `roxmltree` recurses while parsing and overflows the stack on a
50,000-deep document, and a stack overflow aborts the process rather than
unwinding into an error anyone could catch. That was a real one-file denial of
service, found by writing the test above and watching it crash.

Fuzzing proper needs a nightly toolchain and a library target this crate does
not have. In its place the suite carries a seeded mutation pass: valid manifests
and archive headers mutated deterministically and fed to every parser, asserting
only that control comes back. Weaker than a fuzzer at finding new cases,
stronger at one thing — it runs on every `cargo test`.

## Known limits

- **No real mod has been through this tool yet.** Every test uses fixtures
  built from format documentation. The parsing libraries are tested upstream,
  but the integration around them — path heuristics, id fallbacks, game
  detection — is calibrated on invented examples. The corpus harness above
  exists to change that, but it needs a corpus: until someone runs it, treat
  the profiles as informed claims.
- Bannerlord versions are written `v1.0.0`, which semver cannot read, so its
  version requirements come out unverified rather than wrong.
- The XML depth guard is a scanner, not a parser: it over-counts on `>` inside
  attribute values and on angle brackets in comments. Over-counting only makes
  it stricter, which is the safe direction for something whose job is to run
  before a parser that would otherwise crash the process.
- No real fuzzing yet — see above for what stands in for it.

- Record comparison is pairwise and parses every plugin whole, so a very large
  load order costs time and memory. `--no-records` turns it off.
- Plugins are read from disk, so a mod still packed as a `.zip` is not analysed
  at the record level. Creation Engine mods are installed as folders.
- The Creation Engine profile has no load order of its own; it needs a mod
  manager to supply one. Mod Organizer 2 is supported, Vortex is not — see
  above for why.
- Archives nested inside a `.zip` are not expanded — only archives sitting in a
  mod folder are.
- Version comparison is semver. Requirements in another dialect — Forge's
  Maven ranges, for instance — are treated as satisfied rather than guessed at,
  because a false alarm is worse than a miss here.
- Factorio prototype-name collisions live inside `data.lua` and would need a
  Lua parser. Only mod-id level checks today.

## Licence

ModConflict's own source is **MIT** — see [LICENSE](LICENSE).

A *binary* is a different question, because it contains its dependencies.
Record-level detection is built on [`esplugin`](https://crates.io/crates/esplugin),
the plugin parser behind LOOT, which is **GPL-3.0**. A binary linking it must be
distributed under GPL-3.0.

| What | Licence |
|------|---------|
| This repository's source | MIT |
| A binary built with default features | GPL-3.0, because it contains `esplugin` |
| `cargo build --no-default-features` | permissive throughout — no record comparison |

That is what the `records` feature is for. Every other dependency is
permissive. [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) has the full
picture.
