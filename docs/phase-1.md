# Phase 1: offline core

Status: implemented and verified against the 16 real captures in
`samples/items/`.

## Workspace

```
crates/
  poe-data/   Lua subset parser, PoB importer, MessagePack cache
  poe-item/   item text parser, text normalization, affix resolver
  poe-cli/    `poe` binary: data build/info, inspect
```

Build and test:

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Data setup

Fetch the pinned PoB snapshot (paths mirror `src/Data`):

```
python tools/lock-pob-data.py --cache data/pob
```

Build the local cache:

```
cargo run -p poe-cli -- data build `
  --pob-dir data/pob `
  --out target/poe-data.msgpack `
  --lock data/pob-lock.json
```

Inspect the cache:

```
cargo run -p poe-cli -- data info --cache target/poe-data.msgpack
```

Current snapshot: 21,347 mods, 1,271 bases, 18,671 trade stats, 105 essences.
Cache size: about 7.3 MB.

`data/pob` is ignored by git; data files are fetched by commit and never
redistributed in this repository.

## Item inspection

```
cargo run -p poe-cli -- inspect `
  --cache target/poe-data.msgpack `
  samples/items/capture-20261126-123130-advanced-4.txt
```

Options:

- `--all` shows unresolved groups (enchantments, flavor text, veiled
  placeholders).
- `--json` emits the full resolved item.

Output includes item class, name/base, item level, flags, base tags, every
modifier group, matched mod ID, domain, prefix/suffix, game or estimated tier,
roll percentiles, and ties.

## What works

- Advanced copy: headers with and without tags, tiers, ranks, crafted,
  fractured, implicit, enchant, unique, crucible, veiled, and multi-line
  affixes.
- Normal copy fallback: markers `(implicit)`, `(crafted)`, `(fractured)`,
  `(enchant)`, `(scourge)`, `(unscalable)`.
- Sections: properties, requirements, sockets, item level, flavor text, notes,
  influence/state lines, currency, gems.
- Resolution: affix name plus stat text plus roll ranges, base applicability,
  base implicit IDs, domain preference, and flat-value tiers.
- Real captures verified across armour, weapons, flasks, belts, amulets,
  uniques, magic items, and fractured items.

## Known gaps

- `Enchantment*.lua` is not imported, so enchantment groups currently resolve
  as unmatched (`--all`).
- Eldritch implicit tier labels (Lesser/Greater/Grand/Exquisite/Exceptional)
  are captured but not yet used to break all ties.
- Normal-copy tier numbers are estimated by item level and base tags, not read
  from the game.
- Unique mods are matched by text, so identical stats across different uniques
  can tie; crafting targets are rares, so this is low priority.
- Unquoted flavor text on unique items is parsed as unresolved groups; the
  default output hides it.
- `ModMap.lua` contains functions and is skipped; map crafting is out of scope.
- Foulborn, Graft, Mercenary, Synthesis, Scourge, and Necropolis mod files are
  locked but not imported yet.
- MessagePack cache is rebuilt from files; there is no automatic cache
  invalidation yet.

## Next

Phase 2: declarative plan model, goal evaluator, strategy state machine, and a
Monte Carlo simulator over PoB spawn weights.
