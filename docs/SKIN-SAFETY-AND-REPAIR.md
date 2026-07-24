# Skin-mod safety & repair

How Chud detects custom skin mods that break the game, and how it can **repair**
them automatically. Triggered by the 2026-07-23 "Moon Knight Gangplank broke my
abilities" report.

## The one issue that actually breaks the game

A League skin is supposed to be **cosmetic only** — it overrides mesh/texture/VFX
*assets* plus its own *skin records* (`data/characters/<champ>/skins/skin<NN>.bin`).
It must **never** ship the champion's **root character record**
`data/characters/<champ>/<champ>.bin` — that file defines the champion's
**spells, abilities, and stats**.

When a mod does ship it (usually an old, full-champion port), injecting the mod
replaces the game's *live* ability data with the mod's bundled, stale copy. On a
newer patch the abilities break in-game: **missing/unusable spells, can't level,
requires a full client repair.** This is the exact failure Moon Knight caused.

Detector: `skins/injection/target_detect.rs::overrides_ability_data(mod, champ)`
— reads the mod's WAD chunk table and checks for `xxh64("data/characters/
<alias>/<alias>.bin")`. Enforced by the inject-time guard (own + party) and a
proactive Library scan; flagged mods get a BROKEN chip and are never injected.

## Issue taxonomy (what a scan should and shouldn't flag)

Validated by extracting every mod's WAD with the CDTB hashtable and reverse-
mapping the `data/` records. From the reference 28-mod library:

| Class | Breaks game? | Example(s) | Action |
|---|---|---|---|
| **Ability-data override** — ships `<champ>/<champ>.bin` | **YES** | Moon Knight Gangplank, sans-the-skeleton Vayne, Set (Smite) Zaahen (3/28) | Block + repair |
| **Corrupt archive** — bad zip CRC on the WAD entry | Fails to inject | High Noon Gangplank | Repair (re-zip) |
| Animation-graph override — `animations/skin<NN>.bin` | **No** — normal for many working skins | 9/28 incl. Viktor Yuki, Dragon Ball ports | **Ignore** (do NOT flag) |
| Sub-entity data — e.g. `gangplankbarrel` | **No** — it's the champ's own kit | High Noon | Ignore |
| Skin/root records — `skins/skin<NN>.bin`, `skins/root.bin` | **No** — that's the visual | most mods | Keep |
| Foreign-champion data — overrides a *different* champ's `<other>.bin` | **YES** (breaks that champ) | none in library (wild) | Block + strip foreign chunks |
| Shared/global data — `data/shared`, `data/menu`, … | **YES** (broad) | none in library (wild) | Block + strip |

Key correction from the exploratory scan: **animation overrides are benign** —
tons of legit skins include them. Only the **root character record** (and
foreign/shared records) actually break gameplay. The shipped detector already
scopes to exactly the root record, so it flags the 3 real breakers with **zero
false positives** across the 25 working mods.

## Repair engine (proven)

The cslol extract→repack round-trip is **hash-preserving** (verified: identical
75-chunk set before/after on Moon Knight), so we can surgically drop the bad
chunk and repack a game-correct WAD.

Recipe (bundled `resources/cslol-tools/`):
1. Unzip the `.fantome` → `WAD/<Champ>.wad.client`.
2. `wad-extract.exe <wad> <ext> hashes.game.txt` → real paths.
3. Delete the dangerous record(s): `ext/data/characters/<alias>/<alias>.bin`
   (and any foreign/shared records for those classes).
4. `wad-make.exe <ext> <repaired.wad.client>` (absolute paths — relative dst
   fails with "cannot find the path specified").
5. Re-zip: `META/info.json` (preserved) + `WAD/<Champ>.wad.client` (repaired).

**Proven across all three ability-override mods:** Moon Knight 75→74, sans Vayne
334→333, Set Zaahen 321→320 — ability record removed, all skin/asset chunks
retained.

### What repair guarantees
- **Guaranteed safe:** the repaired mod *cannot* break abilities — the game uses
  its own current character record. Worst case the skin fails to load and you see
  the base skin; the game is always fine.
- **Not guaranteed pixel-perfect:** mesh + textures always apply (they're
  assets); VFX recolors that were defined *only* inside the stripped record
  revert to default. For a base-replacement mod the result almost always looks
  right.

### Runtime implementation note
For the in-app repair, a **binary WAD edit** is preferable to shelling out to
cslol + the 227 MB hashtable: compute `xxh64("data/characters/<alias>/
<alias>.bin")`, then rewrite the WAD without that one chunk (drop its 32-byte
table entry, decrement the count, recompute the remaining data offsets). Reuses
the existing `read_wad_hashes` parser in `target_detect.rs`. Self-contained, no
subprocess, no hashtable. cslol extract/make (above) is the reference that
proves the output is correct.

## Planned automation (UI)
- `skins_repair_mod(rel_path)` command → runs the strip-and-repack, writes the
  repaired mod (backing up the original), clears the broken flag, re-verifies.
- A **"Repair skin (protect abilities)"** button on a BROKEN Library row → one
  click converts it and flips BROKEN → WORKING, with a note that abilities are
  protected and the look should carry over (very old mods may render partially).
- Corrupt-archive class: a simpler "re-pack" that re-extracts the WAD raw and
  re-zips a clean `.fantome`.

See also `native/src-tauri/src/skins/broken_mods.rs`,
`native/src-tauri/src/skins/injection/target_detect.rs`.
