# Phase 3: live crafting safety layer

Status: implemented. In-game verification has not been run yet; the session core
is covered by mock tests, and the Windows layer compiles on Windows.

New crates:

- `poe-session` - platform-independent session state machine, budgets,
  verification, logging, and mock test harness.
- `poe-input` - Windows window/process detection, clipboard, SendInput, and
  normalized calibration.

## Safety model

The session treats the clipboard and currency counts as ground truth.

Before each action:

- F12 emergency stop is checked.
- The game window must be foreground.
- The item must parse as normal, magic, or rare with an item class and item
  level.
- The item identity must stay stable across the run: item class, item level,
  sockets, and implicit modifier signature.
- If currency verification is enabled, the currency stack must exist.

After each currency application:

- The currency stack must decrease by exactly one.
- The item must be re-read and still parse.
- The item identity must be unchanged.
- The item text fingerprint must change; if it does not, the run aborts after
  `max_same_fingerprint` repeats.
- The goal is re-evaluated after every currency step.

Budgets:

- `--max-steps` plan actions per run.
- `max_currency_actions = max_steps * 2` (a scour + transmute/regal is one step).
- `--max-seconds`.
- `--max-cost` using the plan's cost table.
- `--once` stops after one plan step.
- Logs are written as JSONL after every verified currency action.

## Calibration

Default path: `.\calibration.toml` in the current working directory.

Capture one target at a time:

```
poe craft calibrate --target item
poe craft calibrate --target alt
poe craft calibrate --target transmute
poe craft calibrate --target aug
poe craft calibrate --target regal
poe craft calibrate --target alch
poe craft calibrate --target scour
poe craft calibrate --target chaos
```

For each command, focus Path of Exile, hover the target slot, and press F8.
Progress is saved immediately. `poe craft calibrate --list` shows captured
points.

Coordinates are normalized to the game client area, so moving the window does
not invalidate them. Use windowed or borderless mode; exclusive fullscreen is
not supported.

Place all currency stacks in the inventory. The target item can be in the stash
or inventory, but the stash/inventory must stay open during the run.

Enable Advanced Mod Descriptions in the game options. Item reads use
`Ctrl+Alt+C`; currency reads use `Ctrl+C`.

## Commands

```
poe craft doctor
poe craft calibrate --list
poe craft run --plan plans\es-boots-alt.toml --dry-run
poe craft run --plan plans\es-boots-alt.toml --once
poe craft run --plan plans\es-boots-alt.toml --yes --max-steps 1000
```

Real runs require typing `YES` unless `--yes` is passed. The CLI waits three
seconds after confirmation so you can focus the game.

`--dry-run` reads the current item, evaluates the goal, and prints the next
action without clicking. It only requires the item slot calibration.

Useful flags:

- `--max-seconds` and `--max-cost` bound a run.
- `--no-currency-verify` disables stack-count checks; item text is still
  verified, but this is less safe.
- `--json` prints the final session report as JSON.

## Currency interaction

For each action the tool:

1. Reads the item and the currency stack.
2. Right-clicks the currency, then left-clicks the item.
3. Left-clicks the currency slot again to return the remaining stack.
4. Re-reads the currency stack and the item, then verifies both.

The return click assumes the game keeps the remaining stack on the cursor after
applying one, which is the normal PoE behavior. If the stack is fully consumed,
the return click lands on an empty slot and is a no-op. Validate the interaction
with `--once` before a long run.

## Emergency stop and failure behavior

- F12 cancels the session.
- Losing game focus aborts.
- Missing calibration aborts before any click.
- A missing currency, unchanged currency count, unreadable item, changed item
  identity, or repeated unchanged fingerprint aborts.
- The tool never retries a failed currency application blindly; it stops so the
  item/cursor state can be inspected manually.

## Known limits

- There is no pixel/OCR check that the stash panel is open. If it closes, the
  first click may land elsewhere and the session aborts afterwards. Keep the
  stash open and use conservative `--once`/dry-run checks.
- Only normal, magic, and rare items are accepted.
- Only the Phase 2 action set is implemented: transmute, alt, aug, regal,
  alchemy, scouring, and chaos. Essence, fossil, bench, eldritch, and veiled
  actions are future work.
- The game process must be discoverable as a PoE1 `PathOfExile*.exe`; launcher
  and Path of Exile 2 windows are rejected. Run the tool at the same privilege
  level as the game.
- The process is system-DPI aware, not per-monitor. Multi-monitor setups with
  different scaling factors may need re-calibration or a single-scale display.
- F8/F12/F12 polling and SendInput are global. Automation is a game Terms of
  Service and ban risk.
- Logs contain raw item text; `/logs/` is ignored by git.
