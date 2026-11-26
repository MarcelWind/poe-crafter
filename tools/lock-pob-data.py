#!/usr/bin/env python3
"""Lock the Path of Building data snapshot used by poe-crafter.

Downloads only the data files needed for crafting, verifies them against the
GitHub tree metadata, and writes data/pob-lock.json with SHA-256 hashes.
Data files are cached outside the repository and are never committed.

Usage:
    python tools/lock-pob-data.py [--ref dev] [--commit SHA] [--out PATH]
                                 [--cache DIR]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

REPO = "PathOfBuildingCommunity/PathOfBuilding"
API = f"https://api.github.com/repos/{REPO}"

SELECTED = {
    # MVP: explicit mods, bases, jewels
    "ModExplicit.lua": "mvp",
    "ModJewel.lua": "mvp",
    "ModJewelAbyss.lua": "mvp",
    "ModJewelCharm.lua": "mvp",
    "ModJewelCluster.lua": "mvp",
    "ClusterJewels.lua": "mvp",
    # Currency domains
    "Essence.lua": "essence",
    "ModDelve.lua": "fossil",
    "ModMaster.lua": "bench",
    "ModVeiled.lua": "veiled",
    "ModCorrupted.lua": "vaal",
    "ModEldritch.lua": "eldritch",
    "ModTincture.lua": "tincture",
    "ModFlask.lua": "flask",
    # Trade / ids / costs
    "TradeSiteStats.lua": "trade",
    "QueryMods.lua": "trade",
    "CurrencyNames.lua": "ui",
    "Costs.lua": "cost",
    # Later / optional
    "ModItemExclusive.lua": "later",
    "ModScalability.lua": "later",
    "ModMap.lua": "later",
    "ModFoulborn.lua": "later",
    "ModGraft.lua": "later",
    "ModScourge.lua": "later",
    "ModSynthesis.lua": "later",
    "ModNecropolis.lua": "later",
    "ModMercenary.lua": "later",
}

EXCLUDED = [
    "ModCache.lua",
    "Gems.lua",
    "Minions.lua",
    "Spectres.lua",
    "WorldAreas.lua",
    "Bosses.lua",
    "BossSkills.lua",
    "BeastCraft.lua",
    "Crucible.lua",
    "DelveDropOnly.lua",
    "IncursionDropOnly.lua",
    "FlavourText.lua",
    "Pantheons.lua",
    "PearlSupports.lua",
    "Rares.lua",
    "SkillStatMap.lua",
    "StatDescriptionFormatters.lua",
    "TattooPassives.lua",
    "Vestigial.lua",
    "Viscera.lua",
    "Misc.lua",
    "ModFoulbornMap.lua",
    "ModFoulbornMap.jsonc",
    "Enchantment*.lua",
]

USER_AGENT = "poe-crafter-data-lock/1"


def get_json(url: str) -> object:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.loads(response.read().decode("utf-8"))


def resolve_commit(ref: str) -> tuple[str, str]:
    data = get_json(f"{API}/commits/{ref}")
    return data["sha"], data["commit"]["committer"]["date"]


def contents(path: str, commit: str) -> list[dict]:
    return get_json(f"{API}/contents/{path}?ref={commit}")


def fetch_file(commit: str, path: str, cache: str) -> bytes:
    local = os.path.join(cache, *path.split("/"))
    if os.path.exists(local):
        return open(local, "rb").read()
    url = f"https://raw.githubusercontent.com/{REPO}/{commit}/{path}"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=120) as response:
        blob = response.read()
    os.makedirs(os.path.dirname(local) or cache, exist_ok=True)
    with open(local, "wb") as handle:
        handle.write(blob)
    time.sleep(0.15)
    return blob


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", default="dev")
    parser.add_argument("--commit")
    parser.add_argument("--out", default=os.path.join("data", "pob-lock.json"))
    parser.add_argument(
        "--cache",
        default=os.path.join(tempfile.gettempdir(), "poe-crafter-pob-cache"),
    )
    args = parser.parse_args()

    if args.commit:
        commit = args.commit
        commit_date = resolve_commit(commit)[1]
    else:
        commit, commit_date = resolve_commit(args.ref)

    data_files = {e["name"]: e for e in contents("src/Data", commit) if e["type"] == "file"}
    base_files = {e["name"]: e for e in contents("src/Data/Bases", commit) if e["type"] == "file"}
    size_by_path = {f"src/Data/{name}": e["size"] for name, e in data_files.items()}
    size_by_path.update({f"src/Data/Bases/{name}": e["size"] for name, e in base_files.items()})

    targets = [(f"src/Data/{name}", purpose) for name, purpose in SELECTED.items()]
    targets += [(f"src/Data/Bases/{name}", "mvp") for name in sorted(base_files)]

    entries = []
    failures = []
    for path, purpose in sorted(targets):
        if path not in size_by_path:
            failures.append(f"{path}: not present in tree metadata")
            continue
        try:
            blob = fetch_file(commit, path, args.cache)
        except (urllib.error.URLError, OSError) as exc:
            failures.append(f"{path}: {exc}")
            continue
        if path in size_by_path and len(blob) != size_by_path[path]:
            failures.append(f"{path}: size {len(blob)} != {size_by_path[path]}")
        entries.append(
            {
                "path": path,
                "size": len(blob),
                "sha256": hashlib.sha256(blob).hexdigest(),
                "purpose": purpose,
            }
        )

    lock = {
        "schema": 1,
        "source": {
            "repo": f"https://github.com/{REPO}",
            "ref": args.ref,
            "commit": commit,
            "commit_date": commit_date,
            "raw_base": f"https://raw.githubusercontent.com/{REPO}/{commit}",
            "license": "MIT (Path of Building); game data (c) Grinding Gear Games",
        },
        "policy": {
            "pinned": True,
            "update": "manual; re-run tools/lock-pob-data.py against a reviewed commit",
            "note": "No data files are redistributed in this repository; fetch by commit and verify sha256.",
        },
        "fetched_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "excluded": EXCLUDED,
        "files": entries,
    }

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(lock, handle, indent=2)
        handle.write("\n")

    print(f"commit {commit} ({commit_date})")
    print(f"locked {len(entries)} files, {sum(e['size'] for e in entries)} bytes")
    if failures:
        print("FAILURES:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print("all size checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
