# Phase 0 findings

Status: complete. Real captures were collected, and Phase 1 (offline data
importer, item parser, resolver, CLI) is implemented; see `docs/phase-1.md`.

## 1. Decision

Build the crafting tool from scratch in Rust. Use ASH only as a behavioral
reference and as a corpus of plan files. Do not decompile or patch the binary.

| Criterion | Reverse engineer `ash.exe` | Build from scratch |
| --- | --- | --- |
| Language | Delphi/Lazarus, stripped, 8.47 MB | Rust, designed for the task |
| Data | PoB snapshot from 2024-03 | Current PoB data (2026-11) |
| Safety | Logic and I/O entangled; cannot verify or fix internals | Pure decision core plus verified I/O |
| UI | Legacy VCL | egui/eframe |
| Extensibility | Fixed currency set and coordinates | Declarative plans, simulator, action registry |
| Effort | High; still requires new data/UI/safety | Moderate; all effort creates product code |

## 2. ASH observations (static)

Binary facts:

- `ash.exe` 8,472,064 bytes, PE version 1.36.4.0.
- Delphi/Lazarus VCL with AlphaControls (`MainForm`, `TForm`, `TTimer`, `Ts*`).
- No .NET, Go, Rust, or Python runtime markers.

Win32 usage (strings):

- Input: `mouse_event`, `SetCursorPos`, `keybd_event`, `SendInput`-style flow.
- Window/screen: `GetForegroundWindow`, `GetClientRect`, `GetWindowRect`,
  `GetWindowDC`, `BitBlt`, `GetPixel`, `GetAsyncKeyState`.
- Memory: `ReadProcessMemory`, `SeDebugPrivilege`.
- `cports.exe` plus `close * * * *` for fast logout by killing the game's TCP
  connection.

Web usage:

- `POESESSID` login, Cloudflare `jschl_vc` challenge parsing.
- `character-window/get-stash-items` for stash scans.
- `api.pathofexile.com/leagues` for the league list.

Data pipeline:

- Parses PoB Lua at runtime: `ModItem.lua`, `ModFlask.lua`, `ModJewel*.lua`,
  `Bases/*.lua`.
- Generates per-domain affix and mod text lists (`*_p.txt`, `*_s.txt`).
- Snapshot in `resources/data/pob` is dated 2024-03-25.

Craft model:

- Plan lines: `[P|S][weight] mod text(min value)`.
- Global thresholds: `weight_magic`, `weight_rare`.
- Currency set: transmute, alt, aug, regal, alch, scour, chaos, plus map crafts.
- Logic switches: magic / regal / alchemy / chaos; augment never / if needed /
  always; use augment before regal.
- Calibration is absolute screen coordinates stored in `settings.ini`.

Infinite-loop root cause:

- The craft loop uses fixed coordinates and blind clicks.
- Screen and input APIs exist, but there is no per-action postcondition such as
  "currency stack decreased" or "item text changed as expected".
- On a parse failure, full affix slots, missing currency, or a lost click, the
  loop can repeat forever.

What we reuse:

- `itemcraft/*` files as compatibility fixtures, not as the plan format.
- No code, no coordinates, no data snapshot, no web session.

## 3. Data source lock

Pinned Path of Building snapshot:

- Repository: `PathOfBuildingCommunity/PathOfBuilding`
- Ref: `dev`
- Commit: `bfee69d10cceef87b7125155645b4d03091ac6ac`
- Commit date: `2026-11-26T02:29:40Z`
- Files locked: 50
- Total size: 16,832,102 bytes
- Manifest: `data/pob-lock.json` (path, size, SHA-256, purpose)
- Fetch/verify: `tools/lock-pob-data.py`

Why PoB:

- Current data, maintained against the live game.
- `ModExplicit.lua` and related files provide `type`, `affix`, level, group,
  `weightKey`/`weightVal`, `modTags`, and `tradeHashes`.
- `tradeHashes` map each mod line to the official trade stat hash, enabling
  canonical text matching and later trade queries without a web session.
- `TradeSiteStats.lua` is a snapshot of the official trade stats endpoint
  (`id`, `text`, `type`).
- `Bases/*.lua` provides base type, class, tags, implicits, and requirements.
- `Essence.lua`, `ModDelve.lua`, `ModMaster.lua`, `ModVeiled.lua`,
  `ModCorrupted.lua`, `ModEldritch.lua` cover later crafting steps.
- License: PoB code/data files are MIT; game data is copyright Grinding Gear
  Games. Data is fetched by commit and not redistributed in this repository.

Why not RePoE:

- Checked `brather1ng/RePoE`: no releases and no tags at inspection time.
- PoB already has current weights, trade hashes, and split domains.

Update policy:

- Manual only, against a reviewed commit.
- Re-run `tools/lock-pob-data.py`; review the manifest diff; bump the data
  version recorded in plans and run logs.
- Never mix two data versions inside one craft run.

## 4. Advanced copy grammar

Verified against the current PoB parser (`src/Classes/Item.lua`) and tests
(`spec/System/TestItemParse_spec.lua`). Fixtures are in `samples/items/`.

Top level:

```
Item Class: Sceptres
Rarity: Unique
Nebulis
Synthesised Void Sceptre
--------
...
```

Sections are separated by `--------`. Relevant sections: properties,
requirements, sockets, item level, enchant/implicit mods, explicit mods,
crucible mods, state lines (`Corrupted`, `Fractured Item`, `Synthesised Item`,
`Mirrored`, `Split`), influence lines, and notes.

Modifier headers (advanced copy):

```
{ Prefix Modifier "Fecund" (Tier: 1) — Life }
{ Suffix Modifier "of the Brute" (Tier: 3) — Attribute }
{ Master Crafted Prefix Modifier "Upgraded" (Rank: 2) — Life, Defences, Armour }
{ Master Crafted Suffix Modifier "of Craft" (Rank: 3) — Elemental, Cold, Resistance }
{ Fractured Prefix Modifier "Thorny" (Tier: 2) — Damage, Physical }
{ Implicit Modifier — Damage, Critical  — 106% Increased }
{ Searing Exarch Implicit Modifier (Lesser) }
{ Corrupted Enhancement }
{ Corrupted Enhancement - Energy Shield }
{ Corruption Enhancement — Mana }
{ Unique Modifier — Attribute  — 19% Increased }
{ Prefix Modifier "Veiled" }
{ Allocated Crucible Passive Skill (Tier: 2) }
```

Rules:

- Parse the quoted affix name when present; it is the most reliable identity.
- Parse `(Tier: N)` for normal affixes, `(Rank: N)` for master crafted.
- Header kind determines prefix/suffix, crafted, fractured, implicit, enchant,
  unique, veiled, or crucible.
- Separator characters are inconsistent: PoB's own Lua pattern matches ASCII
  `-`, while its tests use U+2014 `—`. Accept `-`, `–`, `—`, with one or more
  spaces around them.
- The trailing `N% Increased` is a modifier magnitude, not part of the roll.
  Store it separately.
- `— Unscalable Value` marks a value that must not be scaled.

Roll ranges:

```
+142(130-144) to maximum Life
32(30-37)% increased Spell Damage
Adds 17(16-20) to 35(30-36) Cold Damage to Spells
+35(29-35)% to Cold Resistance
+120(80-100) to all Attributes
```

- Each inline `value(min-max)` gives the roll and its tier range.
- Compute percentile as `(value - min) / (max - min)`; handle negative values,
  reversed bounds, zero-width ranges, and independent rolls on multi-value
  lines.
- Strip the parenthesised range for text matching.

Multi-line affixes:

```
{ Prefix Modifier "Warlock's" (Tier: 4) — Mana, Damage, Caster }
32(30-37)% increased Spell Damage
+46(42-47) to maximum Mana
```

- One header can own several mod lines (hybrid affix).
- Lines continue until the next header, section separator, or state line.
- Treat the group as one affix with multiple stat lines; do not count it twice.

Normal copy:

- No headers, tiers, or ranges.
- Mod lines may end in `(implicit)`, `(crafted)`, `(fractured)`, `(enchant)`,
  `(scourge)`, `(unscalable)`.
- Match text against the mod pool filtered by base tags and item level, then
  solve affix assignment across lines (including hybrids).

Fixtures in `samples/items/`:

| File | Source | Purpose |
| --- | --- | --- |
| `pob-test-life.advanced.txt` | PoB test suite | Single prefix with tier and roll range |
| `pob-test-crafted.advanced.txt` | PoB test suite | Master crafted suffix with rank |
| `pob-test-multiline.advanced.txt` | PoB test suite | Hybrid affix spanning two mod lines |
| `pob-test-nebulis.advanced.txt` | PoB test suite | Full item with implicit, enchant, fractured, crafted, veiled, scourge, crucible, corrupted, and note sections |
| `synthetic-normal-boots.txt` | Hand-written | Normal-copy fallback without headers, tiers, or ranges |

Parser pitfalls confirmed in PoB tests:

- Reminder text in parentheses is not a mod; skip it.
- Section separators reset line context.
- A crafted mod header may include several lines and both prefix/suffix kinds.
- Fractured mods are reordered first by PoB; do not rely on display order.
- Conqueror/unique modifiers may be absent from normal spawn weights; keep a
  fallback candidate list.
- `Item Class:` may be plural (for example `Sceptres`).

## 5. Manual capture protocol

1. In game, enable Advanced Mod Descriptions (Options > UI).
2. Use `Ctrl+Alt+C` on an item for advanced copy; use `Ctrl+C` for a normal
   copy sample.
3. Run:

   ```
   powershell -ExecutionPolicy Bypass -File tools\capture-items.ps1
   ```

4. Capture, in this order:
   - a rare with 4 explicit mods, including one crafted or fractured mod;
   - a magic item with 1-2 mods;
   - a cluster jewel with a notable and small passives;
   - an item with a hybrid (multi-line) affix;
   - one normal `Ctrl+C` copy of any rare.
5. Keep the client in English and in windowed or borderless mode.
6. Record the game patch and league alongside the captures (rename or note).

The captures decide the exact dash character and any 2026 format changes. If
the headers differ from the PoB tests, update the parser and this document.

## 6. Phase 1 entry criteria

- Data manifest is pinned and verified (done).
- Advanced-copy fixtures exist for singles, hybrids, fractured, crafted,
  implicit, enchant, and multi-section items (done).
- At least one real advanced copy and one real normal copy are captured
  (manual).
- Then start `poe-data` (Lua importer + cache), `poe-item` (parser + matcher),
  and a CLI inspector that prints affixes, tiers, roll percentiles, and goal
  evaluation.

## 7. Open risks

- Real client may use ASCII `-` instead of `—`; parser accepts both.
- Localization: English client text only for now.
- Advanced copy may omit or alter fields after a game patch; fixtures and a
  schema check must catch that.
- Tier numbering from the game must be reconciled with computed item-level
  tiers for normal-copy fallback.
- PoB data can contain legacy and league-specific mods; filter by domain,
  item level, and base tags.
- Automation is a Terms of Service and ban risk; keep conservative delays,
  budgets, and a semi-auto mode.
