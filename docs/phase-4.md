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

## UI tabs

- **Doctor** - game window, data cache, calibration, and window status.
- **Calibrate** - item, currency, and optional panel guards.
- **Inspect** - load or paste item text and resolve affixes.
- **Plan** - validate a plan and run a bounded simulation.
- **Run** - dry run or real run, budgets, live events, final report, and Stop.

The real run executes on a worker thread. Progress and completion flow back to
the UI through a channel; Stop sets the session's cancel flag. The game is
focused automatically before the session starts.

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

`CraftSession::set_cancel_flag` accepts an `Arc<AtomicBool>`. The gate checks it
together with F12, foreground state, and preflight. The UI's Stop button sets
the same flag.

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
