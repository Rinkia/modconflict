# Contributing

Thanks for looking. This file is short on ceremony and long on the two or three
rules that actually matter here.

## The one invariant

**ModConflict reads. It never writes to a mod folder, a save, or a mod
manager's own files.** Every feature request that would change a user's
installation is out of scope, however useful — this tool tells you what will
break, and you decide what to do about it. Sorting load order is LOOT's job;
installing mods is the manager's.

## Getting set up

```bash
git clone https://github.com/Rinkia/modconflict
cd modconflict
cargo test
cargo clippy --all-targets
```

No game and no mods required: every test builds its own fixtures — real zips,
real `.bsa` archives, real Larian `.pak` files — in a temp directory.

One extra build must keep passing, because it is what makes the MIT claim on
the source true:

```bash
cargo build --no-default-features
```

See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for why.

## Adding a game

This is the most useful contribution and usually the smallest. **A game is a
data file, not code.** Look at `profiles/*.toml` and the profile reference in
the README.

The rule the test suite enforces: **a profile without a fixture does not go in.**
A wrong profile does not crash — it parses nothing, falls back to filenames and
cheerfully reports a clean folder, which is worse than no support at all. So:

```
profiles/<game>.toml                    the profile
profiles/fixtures/<game>/input/<file>   a sample manifest
profiles/fixtures/<game>/expected.json  the exact id, version, provides, requires
```

`expected.json` carries a `source_of_truth` field naming where your
understanding of the format comes from — a wiki page, official docs, a
specification. It exists so a wrong fixture can be traced instead of argued
about.

Then run the tool against a folder of that game's mods you actually have, and
check the coverage line:

```
warning: read metadata for only 3 of 40 mods (8%) — ...
```

That number is the difference between a profile that works and one that only
looks like it does.

If the game's metadata is binary, or needs path predicates to navigate, a
profile is the wrong tool — see `metadata_reader` in `src/bg3.rs` for the seam
that exists for exactly that case.

## Adding an archive format

`src/container.rs` holds a table of formats; each is a sniff function and a read
function. Prefer an existing crate over hand-written parsing — the crates track
each format's version drift, which is the part that actually rots. Archives are
identified by magic bytes rather than extension, because Baldur's Gate 3 and
Unreal both use `.pak` for entirely unrelated formats.

## What the tests are for

- **Unit tests** live beside the code they test.
- **`profiles/fixtures/`** proves each profile matches the format as documented.
- **`tests/snapshots/`** freezes the exact text and JSON a few known folders
  produce. If your change alters output, the diff will show it; regenerate with
  `UPDATE_SNAPSHOTS=1 cargo test snapshot` and put the diff in your PR so a
  reviewer can see what changed for users.
- **`src/hostile.rs`** feeds the parsers malformed and malicious input. Every
  file this tool opens was downloaded from the internet; an error or a warning
  is fine, a panic is a bug.
- **`src/corpus.rs`** runs against real mod folders on your machine and is
  skipped otherwise. If you have mods, please run it — see the README. It is
  the only thing that checks the profiles against what modders actually publish
  rather than what the docs claim.

## Style

Match the code around you. Two habits worth knowing:

- **Comments explain *why*, never *what*.** If a line needs explaining, it is
  usually the line that should change. The comments worth writing are the ones
  recording a decision someone would otherwise undo: why file overlap is a
  warning and not an error, why a limit is checked before a parser rather than
  after.
- **Never index untrusted text by byte.** `strip_prefix`, not `split_at(1)`. A
  `modlist.txt` with a UTF-8 BOM panicked this tool exactly once, and PowerShell
  writes a BOM by default.

Run `cargo clippy --all-targets` before opening a PR. It is clean today and
should stay that way.

## Commits and pull requests

Conventional commit subjects (`feat:`, `fix:`, `docs:`, `refactor:`). Say *why*
in the body, not just what — the diff already says what.

A pull request wants: what changed, what you ran, and what you could not verify.
That last one is the valuable part. "I could not test this against a real
Bannerlord install" is genuinely useful information, and much better than
silence.

## Where the project is honest about itself

The README has a **Known limits** section and it is kept current. If you hit
something it does not mention, that gap is a bug report worth filing on its own.
