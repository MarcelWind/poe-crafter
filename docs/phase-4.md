# Phase 4: desktop shell and pre-click safety

Status: initial implementation. The UI compiles and exercises the same
`poe-session` engine as the CLI; in-game UI verification is still manual.

## What changed

- New `poe-ui` crate using `eframe/egui`, launched with `poe-ui`.
- New `Win32GameIo` adapter shared by the CLI and UI.
- Optional `panel` calibration guard: a sampled client-area pixel must match
  before a currency pickup or item click.
- `GameIo::preflight` runs before every session gate.
- `CraftSession` now exposes live `SessionProgress` events and an external
  cancel flag.
- The session releases held Shift on normal completion, errors, and
  cancellation.
- Missing data caches are downloaded and built automatically by the CLI and UI;
  files are verified against `data/pob-lock.json` before importing.
- The UI checks the latest GitHub release on startup and on demand, stages a
  binary-only update, and restarts through the new binary in helper mode.
  Config, plans, calibration, logs, and data stay in place.

## UI tabs

- **Doctor** - game window, data cache, calibration, and window status.
- **Calibrate** - item, currency, and optional panel guards.
- **Inspect** - load or paste item text and resolve affixes.
- **Plan** - validate a plan and run a bounded simulation.
- **Run** - dry run or real run, budgets, live events, final report, and Stop.
- **Settings** - start/stop/calibrate/copy hotkeys, saved to `config.toml`.

The real run executes on a worker thread. Progress and completion flow back to
the UI through a channel; Stop sets the session's cancel flag. The game is
focused automatically before the session starts.

A global start-hotkey monitor runs while the UI is open, so the start key works
with Path of Exile focused. Stop, calibrate, and copy shortcuts are applied to
the shared input layer used by the CLI and UI.

## Panel guard

Calibrate `panel` on a stable pixel of the open stash or inventory, not on the
item or a currency slot. The tool samples that pixel from the composed desktop
at client coordinates and compares it with the captured color within a
tolerance.

The guard is optional so existing calibrations keep working. When it is absent
the UI shows a warning and the session falls back to item/clipboard checks. When
present, a mismatch aborts with a preflight error before the next click.

This is a single-pixel probe, not OCR or template matching. It detects a closed
panel but not every overlay or UI state. Keep the stash open and use
conservative runs.

## Session events

`CraftSession::set_progress_sink` receives:

- `Started` after the first item read;
- `Applied` after each verified currency action, with the evaluation and any
  warning.

The UI also checks updates in the background, so a newer release can replace the
two executables and restart without touching user data.


`CraftSession::set_cancel_flag` accepts an `Arc<AtomicBool>`. The gate checks it
together with the configured stop hotkey, foreground state, and preflight. The
UI's Stop button sets the same flag.

## Updates

`poe-update` reads GitHub's latest release, compares its `vX.Y.Z` tag with the
running package version, downloads `poe-crafter-windows-x64.zip`, and extracts
only `poe-ui.exe` and `poe.exe` into a temporary staging directory. The new
binary is launched with `--apply-update`, waits for the old process, replaces
the two binaries, and starts the UI from the install directory. User config,
plans, calibration, logs, and data are not touched.

Settings has the startup-check toggle, current version, and a manual check
button. Doctor also has a manual check button. Debug builds report available
releases but do not install them unless `POE_CRAFTER_ALLOW_DEBUG_UPDATE=1` is
set. See `docs/releasing.md` for the release workflow.

## Data setup

On startup the UI checks `target/poe-data.msgpack`. If it is missing, a worker
downloads the pinned files listed in `data/pob-lock.json` to `data/pob`, verifies
size and SHA-256, imports them, and builds the cache. The Doctor tab shows
progress and exposes a retry button. The CLI performs the same check before any
command that needs item or plan data.

## Remaining work

- Full panel/template detection and cursor-item detection.
- More crafting actions beyond the Phase 2 currency set.
- A general plan/step DSL instead of fixed strategy methods.
- Batch crafting: multiple slots, stash scanning, and item queues.


## Build tab

The **Build** tab authors executable schema 2 plans:

- choose an item class/base, item level or unrestricted, and up to two classic influences;
- add ordered steps (`alt`, `alt_aug`, `regal`, `scour_alch`, `chaos`, and single currency actions);
- search the eligible pool and add required or weighted mods with tier/value thresholds;
- validate the base, action sequence, slot caps, and step goals;
- show analytic or simulated per-step success probability, expected attempts, and confidence attempts;
- save atomically to the plan path used by the Plan and Run tabs.

Schema 1 plans remain available in the Plan tab and are not editable in the builder.
