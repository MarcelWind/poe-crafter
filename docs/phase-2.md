# Phase 2: plans, goals, and Monte Carlo simulation

Status: implemented and verified against the pinned PoB data and the real
captures.

New crate: `poe-craft` (`plan`, `state`, `eval`, `pool`, `sim`, `rng`).

## What it does

- Loads crafting plans from TOML with strict schema validation.
- Resolves a concrete base and builds the eligible explicit-mod pool at the
  plan's item level, using PoB spawn weights and base tags.
- Computes tiers from the eligible pool and rolls tier-appropriate values.
- Evaluates goals over either a parsed item (`plan check`) or a simulated item.
- Runs deterministic Monte Carlo simulations for alt/aug/regal, chaos spam, and
  alchemy/scour loops.
- Rejects impossible plans before any simulation: unknown groups, tiers that
  cannot roll, and unreachable values.
- Recommends the next action for a real item.

## Example plans

- `plans/es-boots-alt.toml` - two prefixes on Sorcerer Boots via alt + regal.
- `plans/chaos-ring.toml` - life + cold resistance on an Amethyst Ring.
- `plans/magic-cold-res.toml` - single cold resistance suffix via alt spam.

```toml
schema = 1
name = "ES boots: two prefixes (alt + regal)"

[base]
item_class = "Boots"
base_names = ["Sorcerer Boots"]
item_level = 86

[goal]
min_prefixes = 2
max_prefixes = 2

[[goal.required]]
slot = "prefix"
group = "LocalEnergyShieldPercent"
min_tier = 2

[[goal.required]]
slot = "prefix"
group = "IncreasedLife"
min_value = 80

[strategy]
method = "alt_aug_regal"
max_attempts = 50000
augment = "never"
regal_when_score = 1
magic_two_mod_chance = 0.5

[costs]
transmute = 0.01
alt = 0.05
aug = 0.1
regal = 0.8
scour = 0.08
```

## Goal semantics

Counts:

- `min_prefixes`, `max_prefixes`, `min_suffixes`, `max_suffixes`.
- `min_open_prefixes`, `min_open_suffixes`.

Requirements:

- `goal.required` entries must all match distinct affixes.
- `goal.weighted` entries are optional; each match adds its `weight`.
- `min_score` is checked against required + weighted matches.
- Required entries also contribute their weight (default 1) to the score, so
  `regal_when_score = 1` means "regal once one desired affix is present".
- Each mod can satisfy at most one requirement.
- Required entries are counted toward slot caps and `max_prefixes`/`max_suffixes`
  during validation.
- `weighted` is capped at 12 entries and each weight at 1,000,000, keeping the
  score search bounded.
- `plan check` warns when the item class or base does not match the plan target.

Requirement fields:

- `slot`: `any`, `prefix`, or `suffix`.
- `group`: PoB mod group (recommended, stable across tiers).
- `affix`: exact affix name.
- `min_tier`: accept tier <= this value (T1 is best).
- `min_value`: minimum value at `value_index`.
- `min_percentile`: minimum roll percentile (0..1) at `value_index`.
- `value_index`: index into the flattened stat values of the mod (default 0).
- `weight`: score contribution (default 1).
- `label`: optional display label used by `plan validate`.

`plan groups --search ...` lists group names for authoring.

## Strategy semantics

Methods:

- `alt_aug_regal`: alt the magic item, optionally augment, regal; on regal
  failure scour and restart.
- `alt_regal`: same without augment.
- `chaos`: ensure a rare item, then chaos-reroll until the goal is met.
- `alch_scour`: alchemy to rare, check, scour, repeat.

Parameters:

- `max_attempts`: number of evaluated rerolls per run (also a hard cap).
- `augment`: `never`, `if_needed`, or `always` (alt_aug_regal only).
- `regal_when_score`: regal a magic item once its partial score reaches this.
  This uses matched affixes even when other required affixes are still missing.
- `magic_two_mod_chance`: probability that alt produces two mods.
- `chaos_mod_counts`: possible mod counts for chaos rerolls (default 4,5,6).
- `start`: `normal`, `magic`, or `rare`.

Costs are arbitrary units, normally chaos-equivalent.

## Commands

Validate a plan:

```
cargo run -p poe-cli -- plan validate `
  --plan plans\es-boots-alt.toml
```

Simulate:

```
cargo run -p poe-cli -- plan simulate `
  --plan plans\es-boots-alt.toml `
  --runs 1000 --seed 42
```

Simulation is bounded by `--max-total-attempts` (default 100,000,000;
`0` means unlimited). Progress goes to stderr, and the report distinguishes
requested runs from completed runs when the budget stops the run early.

Evaluate a real item and recommend an action:

```
cargo run -p poe-cli -- plan check `
  --plan plans\es-boots-alt.toml `
  samples\items\capture-20261126-123149-advanced-8.txt
```

Search groups while writing a plan:

```
cargo run -p poe-cli -- plan groups --search "maximum life" --limit 20
```

All commands accept `--cache target\poe-data.msgpack`; `simulate` and `check`
accept `--json`.

## Verified results (seed 42)

- Magic cold resistance ring: 5,000 runs, mean 63.8 attempts, mean cost 3.15.
- ES boots (two prefixes): 1,000 runs, mean 518 attempts, mean cost 57.6;
  success rate 100% within the 50k attempt cap.
- Chaos ring (life + cold resistance): 1,000 runs, mean 155 chaos.

Simulation results depend on the model parameters below and are estimates, not
game-exact probabilities.

## Model assumptions and limits

- Only `ModExplicit` mods are used for rolling; master crafts, essences,
  fossils, eldritch currency, and veiled mods are not yet simulated.
- Spawn weight uses the first matching weight key; PoE's internal selection is
  approximated.
- Tier numbers are computed relative to the mods eligible for the chosen base
  and item level, so they can differ from the in-game tier when a higher tier is
  excluded by base-specific weights.
- Alt uses a configurable two-mod chance; alchemy uses four mods; chaos uses the
  configured count distribution. Augment and regal choose the side by relative
  pool weight.
- Rolls are uniform; integer ranges roll integers, other ranges roll continuous
  values. Flat stats have percentile 1.0.
- Costs are user-supplied and do not include trade friction or currency
  acquisition.
- Plans currently use a fixed strategy method; a general step DSL is future
  work.
- The attempt budget is checked between runs, so the final started run can
  overshoot it by that run's own attempt count.
- Input automation, verification, and safety budgets are Phase 3.

## Performance notes

Hot-path allocations were removed from the simulator: pool picks now return
borrowed mods, per-roll used-group sets use fixed stack arrays, reward-state
strings are not copied twice, and goal evaluation uses a stack-backed used-mod
mask for normal item sizes. Release timings on the development machine:

- chaos ring, 10,000 runs (~1.55M attempts): about 4.8 s
- ES boots, 10,000 runs (~5.2M attempts): about 4.3 s
- magic cold resistance, 50,000 runs (~3.4M attempts): about 2.3 s

## Phase 3 entry

Use the simulator to choose plans, then implement Win32 input with verified
postconditions, calibration, budgets, and an emergency stop so a real craft
session can run the same state machine safely.
