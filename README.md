# PoE Crafter

A lightweight Path of Exile 1 crafting planner and verified live-crafting tool.
It parses advanced item copies, builds crafting plans from pinned Path of
Building data, simulates outcomes, and can run a calibrated currency loop with
postcondition checks.

This is not affiliated with Grinding Gear Games. Automation is a game Terms of
Service and ban risk. Test with `--dry-run` and `--once` first.

## Quick start

```powershell
# Fetch and build the local PoB data cache.
python tools/lock-pob-data.py --cache data/pob
cargo run -p poe-cli -- data build `
  --pob-dir data/pob `
  --out target/poe-data.msgpack `
  --lock data/pob-lock.json

# Inspect an item and validate a plan.
cargo run -p poe-cli -- inspect `
  --cache target/poe-data.msgpack `
  samples/items/capture-20261126-123149-advanced-8.txt
cargo run -p poe-cli -- plan validate `
  --plan plans/es-boots-alt.toml

# Desktop UI.
cargo run -p poe-ui
```

The UI and CLI use `target/poe-data.msgpack`, `calibration.toml`, `delays.toml`,
and `plans/*.toml` by default. Local calibration, delays, logs, and the data
directory are git-ignored.

## Calibration

Enable Advanced Mod Descriptions in the game, keep the client in English, and
use windowed or borderless mode. Capture each target by focusing Path of Exile,
hovering the target slot, and pressing F8:

```
poe craft calibrate --target item
poe craft calibrate --target panel
poe craft calibrate --target alt
poe craft calibrate --target transmute
poe craft calibrate --target aug
poe craft calibrate --target regal
poe craft calibrate --target alch
poe craft calibrate --target scour
poe craft calibrate --target chaos
```

`panel` is optional but recommended: hover a stable pixel on the open
stash/inventory panel. The run aborts before the next action if that pixel no
longer matches. Without it, a closed panel is not detected until the item read
fails.

## Live crafting

```
poe craft run --plan plans/es-boots-alt.toml --dry-run
poe craft run --plan plans/es-boots-alt.toml --once
poe craft run --plan plans/es-boots-alt.toml --max-steps 1000
```

The default currency flow picks up the stack with a right-click, holds Shift,
and left-clicks the item once per use. It verifies the item identity and text
fingerprint after every use; currency counts are compared when the stack is
released. F12 stops a run and releases Shift.

The action delay is one clamped Gaussian profile:

```toml
[action]
mean_ms = 60
deviation_ms = 10
min_ms = 50
max_ms = 70
```

## Workspace

- `poe-data` - PoB Lua importer and MessagePack cache.
- `poe-item` - item parser, text normalization, and affix resolver.
- `poe-craft` - plans, goal evaluation, mod pools, and Monte Carlo simulation.
- `poe-session` - platform-independent verified session state machine.
- `poe-input` - Windows window, clipboard, input, and calibration layer.
- `poe-timing` - shared RNG and Gaussian delay profiles.
- `poe-cli` - `poe` command-line interface.
- `poe-ui` - `poe-ui` desktop shell.

## Validation

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

See `docs/phase-3.md` and `docs/phase-4.md` for the safety model and UI notes.
