# Phase 4: desktop shell and pre-click safety

Status: initial implementation. The UI compiles and exercises the same
`poe-session` engine as the CLI; in-game UI verification is still manual.

## What changed

- New `poe-ui` crate using `eframe/egui`, launched with `poe-ui`.
- New `Win32GameIo` adapter shared by the CLI and UI.
- Optional template-based panel guard: a grayscale, translation/scale-tolerant
  NCC match must pass before a currency pickup or item click. The legacy
  single-pixel `panel` probe still works as a fallback.
- Configured stash tabs with horizontal/vertical strip ROIs, per-tab
  selected/unselected templates, and grid-addressed item sources.
- Dynamic item anchor: the UI captures the hovered item from the start hotkey;
  the CLI uses `--item-at-cursor` with the calibrate hotkey.
- `GameIo::preflight` runs before every session gate and is click-free.
  `GameIo::prepare_source` performs bounded tab navigation only before a
  currency is picked up.
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

- **Doctor** - game window, data cache, guard/template status, tabs, and window status.
- **Calibrate** - legacy item/currency points plus a screenshot ROI editor for
  the image guard, tab strips, tab content grids, tab templates, and item
  source mapping.
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

## Vision guard and stash tabs

The preferred guard is an image template of stable stash or inventory chrome.
Capture the client from the Calibrate tab, drag a rectangle over chrome that
does not change with tab or slot contents, and apply it as the panel guard.
Before every action the tool captures that region from the composed desktop,
converts both sides to grayscale, blurs, and runs a normalized cross-correlation
search over a small translation window and a few scales. A mismatch aborts
before the next click and saves the live crop plus score under `logs/`.

The legacy single-pixel `panel` probe remains supported for old calibrations
and is used only when no image guard is configured. Real runs without either
guard still work after a warning; dry runs remain permissive.

Stash tabs are configured in `calibration.toml` schema 2:

- horizontal and/or vertical tab-strip ROI;
- per-tab index, kind/name, content grid (columns/rows), and selected and
  unselected header templates;
- item sources as either an inventory point or `(tab, column, row)`.

Template paths are relative and must stay inside the calibration directory;
absolute, UNC, and `..` paths are rejected. Saving also refuses a guarded or
tabbed calibration when a referenced template file is missing, so capture the
selected template before expecting the saved config to run.

`prepare_source` identifies the selected tab from the selected templates,
finds the target tab with its unselected/selected template, clicks the match
center, verifies the selected template, and scrolls the identified strip with
bounded retries if needed. It is only called before picking up or applying
currency, never while a stack is held. The hide/show button for the vertical
strip is not automated.

Before clicking a tab or currency slot, `read_currency` copies the hovered
stack and verifies its display name against the configured name. This catches
wrong-tab and wrong-cell layouts even if template matching drifts.

## Item anchor

The fixed item slot is optional. The UI start hotkey captures the cursor
position while the item is hovered and uses it as the item anchor for that run.
The CLI uses `--item-at-cursor` and asks for the calibrate hotkey after focusing
the game; without it, a legacy calibrated `item` point is used.

Tab switching changes what the stash shows. An item inside a stash tab is only
accepted when every tab source used by the plan resolves to that same tab
(no switch is required); otherwise place the item in the player inventory.

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

- Cursor-item detection and OCR for arbitrary slot text.
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
