# PoE Crafter

A lightweight Path of Exile 1 crafting planner and verified live-crafting tool.
It parses advanced item copies, builds crafting plans from pinned Path of
Building data, simulates outcomes, and can run a calibrated currency loop with
postcondition checks.

This is not affiliated with Grinding Gear Games. Automation is a game Terms of
Service and ban risk. Test with `--dry-run` and `--once` first.

## Quick start

```powershell
# Desktop UI. The first run downloads and builds the pinned data cache.
cargo run -p poe-ui

# Inspect an item and validate a plan.
cargo run -p poe-cli -- inspect `
  --cache target/poe-data.msgpack `
  samples/items/capture-20261126-123149-advanced-8.txt
cargo run -p poe-cli -- plan validate `
  --plan plans/es-boots-alt.toml
```

If `target/poe-data.msgpack` is missing, the CLI and UI automatically:

1. Read `data/pob-lock.json`.
2. Download missing pinned files from the locked Path of Building commit into
   `data/pob`.
3. Verify each file’s size and SHA-256.
4. Import the data and write the cache.

Existing valid files are reused, so an interrupted download resumes. An
existing cache is never overwritten automatically; delete it to rebuild.

Offline/manual builds remain available:

```powershell
python tools/lock-pob-data.py --cache data/pob
cargo run -p poe-cli -- data build `
  --pob-dir data/pob `
  --out target/poe-data.msgpack `
  --lock data/pob-lock.json
```

The UI and CLI use `target/poe-data.msgpack`, `calibration.toml`, `delays.toml`,
and `plans/*.toml` by default. Local calibration, delays, logs, and the data
directory are git-ignored.

## Hotkeys

Copy `config.example.toml` to `config.toml` or use the UI Settings tab. Defaults:

```toml
[hotkeys]
start = "F9"
stop = "F12"
calibrate = "F8"
copy = "Ctrl+C"
```

- **Start** starts the current UI run. It is global while `poe-ui` is running,
  so it works while Path of Exile is focused.
- **Stop** cancels a CLI or UI session and releases held Shift.
- **Calibrate** captures a point while Calibrate/`craft calibrate` is waiting.
- **Copy** is sent to read the item and currency stacks.

Shortcut names support letters, digits, `F1`-`F24`, modifiers (`Ctrl`, `Alt`,
`Shift`, `Win`), and common keys such as `Space`, `Enter`, `Escape`, arrows, and
navigation keys. `config.toml` is local and git-ignored.

The same file controls the startup update check:

```toml
[update]
check_on_startup = true
```

## Auto-updates

`poe-ui` checks the latest stable GitHub release on startup. A newer release is
downloaded and staged, then a helper waits for the old UI to exit, replaces only
`poe-ui.exe` and `poe.exe`, and restarts the app. `config.toml`, calibration,
delays, plans, logs, and the data cache are preserved. Use **Settings > Check
for updates** or the Doctor button to check manually; disable the startup check
in Settings with `update.check_on_startup = false`.

Debug builds check but do not install updates. Set
`POE_CRAFTER_ALLOW_DEBUG_UPDATE=1` only when testing the updater from a debug
build. The first release with the updater must be installed manually.

Release publishing is documented in `docs/releasing.md`.

## Calibration

Enable Advanced Mod Descriptions in the game, keep the client in English, and
use windowed or borderless mode. Capture each target by focusing Path of Exile,
hovering the target slot, and pressing the calibrate hotkey (F8 by default):

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
released. The configured stop hotkey cancels the run and releases Shift.

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
- `poe-update` - GitHub release checks and the Windows self-updater.
- `poe-cli` - `poe` command-line interface.
- `poe-ui` - `poe-ui` desktop shell.

## Validation

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

See `docs/phase-3.md` and `docs/phase-4.md` for the safety model and UI notes.
