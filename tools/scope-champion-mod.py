"""Scope a champion-skin .fantome to its target WAD so cslol mkoverlay
doesn't rebuild half the game.

Community skin mods (especially RAW/loose-file ones) often include copies of
widely-shared game assets - default particle textures, item-recommendation
metadata, generic glows - that exist in hundreds of game WADs. cslol merges
every mod entry into EVERY game WAD containing that entry's path-hash, so one
lazy `assets/shared/particles/defaultcoloroverlifetime.tex` (present in 227
WADs) turns a single-champion skin into a 20+ GB, 2+ minute full-game overlay
rebuild (measured on "Rouxls Kaard Twisted Fate": 205 WADs / 22 GB / 164 s
raw -> 6 WADs / 423 MB / 1 s scoped).

Rule per entry (path-hash xxh64 of the lowercased path):
  KEEP  if the hash is unknown to the game (a brand-new file) or present in
        <= MAX_ENTRY_WADS game WADs - specifically-targetable content: the
        champion's own WAD, its X.<locale>.wad.client VO sibling, event twins
        (Strawberry_/Ruby_), or ANOTHER champion in a multi-champion pack
        (ownership tests are wrong: an "Ahri" pack can legitimately carry
        Akali files living in exactly 1 WAD)
  DROP  only entries present in many WADs - shared assets the game already
        has everywhere (default particle textures in 200+ WADs,
        item_metadata.rec in 173...); a skin never intends a global override.

Handles both packed (`WAD/X.wad.client` file member) and RAW WAD-folder
fantomes; output is always packed via the bundled wad-make. A `.bak` copy is
written beside the source on first run.

Usage: python tools/scope-champion-mod.py "<mod.fantome>" [more mods...]
Requires: pip install xxhash; a League install at the standard path (or set
LEAGUE_GAME_DIR); Chud's cslol-tools in %LOCALAPPDATA%\\Chud\\cslol-tools.
"""

import collections
import glob
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

import xxhash

GAME_DIR = os.environ.get("LEAGUE_GAME_DIR", r"C:\Riot Games\League of Legends\Game")
TOOLS = os.path.join(os.environ["LOCALAPPDATA"], "Chud", "cslol-tools")
# An entry in <= this many game WADs is specifically-targetable content;
# beyond it, shared junk (catastrophic entries live in 34-227 WADs).
MAX_ENTRY_WADS = 5


def wad_toc(path):
    try:
        with open(path, "rb") as f:
            if f.read(2) != b"RW":
                return set()
            f.seek(268)
            (count,) = struct.unpack("<I", f.read(4))
            data = f.read(count * 32)
            return {struct.unpack_from("<Q", data, i * 32)[0] for i in range(count)}
    except OSError:
        return set()


def build_game_index():
    counts = collections.Counter()
    by_name = {}
    for w in glob.glob(os.path.join(GAME_DIR, "DATA", "FINAL", "**", "*.wad.client"), recursive=True):
        hs = wad_toc(w)
        by_name[os.path.basename(w).lower()] = hs
        for h in hs:
            counts[h] += 1
    if not counts:
        sys.exit(f"no game WADs found under {GAME_DIR} - set LEAGUE_GAME_DIR")
    return counts, by_name


def path_hash(rel):
    stem = os.path.basename(rel).rsplit(".", 1)[0]
    if len(stem) == 16 and all(c in "0123456789abcdef" for c in stem.lower()):
        return int(stem, 16)
    return xxhash.xxh64(rel.replace("\\", "/").lower().encode()).intdigest()


def scope_wad_folder(src_dir, counts):
    """Delete entries in src_dir that fail the scoping rule. Returns (kept, dropped)."""
    kept, dropped = 0, []
    for root, _, files in os.walk(src_dir):
        for fn in files:
            full = os.path.join(root, fn)
            rel = os.path.relpath(full, src_dir).replace("\\", "/")
            if rel == "hashed_files.json":
                continue
            h = path_hash(rel)
            if counts[h] <= MAX_ENTRY_WADS:
                kept += 1
            else:
                dropped.append((counts[h], rel))
                os.remove(full)
    return kept, dropped


def scope_fantome(path, counts, by_name):
    with tempfile.TemporaryDirectory(prefix="chud_scope_") as tmp:
        with zipfile.ZipFile(path) as z:
            z.extractall(tmp)
        wad_root = os.path.join(tmp, "WAD")
        if not os.path.isdir(wad_root):
            print(f"SKIP {path}: no WAD/ directory")
            return False

        total_kept, total_dropped = 0, []
        out_wads = []
        for member in sorted(os.listdir(wad_root)):
            if not member.lower().endswith(".wad.client"):
                continue
            member_path = os.path.join(wad_root, member)

            if os.path.isfile(member_path):  # packed -> extract to filter
                extracted = os.path.join(tmp, "_x", member)
                subprocess.run(
                    [os.path.join(TOOLS, "wad-extract.exe"), member_path, extracted,
                     os.path.join(TOOLS, "hashes.game.txt")],
                    check=True, capture_output=True,
                )
                os.remove(member_path)
                member_path = extracted

            kept, dropped = scope_wad_folder(member_path, counts)
            total_kept += kept
            total_dropped.extend(dropped)
            out_wads.append((member, member_path, None))

        if not out_wads:
            print(f"SKIP {path}: no *.wad.client members under WAD/")
            return False
        if not total_dropped:
            print(f"OK   {path}: already scoped ({total_kept} entries) - unchanged")
            return True

        bak = path + ".bak"
        if not os.path.exists(bak):
            shutil.copy2(path, bak)

        new_zip = path + ".new"
        with zipfile.ZipFile(new_zip, "w", zipfile.ZIP_DEFLATED) as zout:
            meta = os.path.join(tmp, "META")
            for root, _, files in os.walk(meta):
                for fn in files:
                    full = os.path.join(root, fn)
                    zout.write(full, os.path.relpath(full, tmp).replace("\\", "/"))
            for member, member_path, _ in out_wads:
                if os.path.isdir(member_path):
                    packed = os.path.join(tmp, "_packed_" + member)
                    subprocess.run(
                        [os.path.join(TOOLS, "wad-make.exe"), member_path, packed],
                        check=True, capture_output=True,
                    )
                    member_path = packed
                zout.write(member_path, f"WAD/{member}")
        os.replace(new_zip, path)

        print(f"OK   {path}: kept {total_kept}, dropped {len(total_dropped)} shared entries (backup: {bak})")
        for c, rel in sorted(total_dropped, reverse=True)[:8]:
            print(f"       dropped ({c} wads): {rel}")
        return True


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    counts, by_name = build_game_index()
    ok = True
    for p in sys.argv[1:]:
        ok = scope_fantome(p, counts, by_name) and ok
    sys.exit(0 if ok else 1)
