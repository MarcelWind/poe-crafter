# Releasing and auto-updates

The desktop client checks the repository's latest GitHub release on startup and
can also check from Settings or Doctor. A newer stable release is downloaded,
verified as a ZIP, staged outside the install directory, and installed by the
new `poe-ui.exe` in helper mode.

## Client behavior

- Current version comes from the workspace package version.
- The latest release is read from
  `https://api.github.com/repos/MarcelWind/poe-crafter/releases/latest`.
- The asset must be named `poe-crafter-windows-x64.zip` (a
  `*-windows-x64.zip` fallback is accepted).
- The updater extracts only `poe-ui.exe` and `poe.exe` from the ZIP.
- The running process waits for the old process to exit, replaces those two
  binaries, and starts the updated UI from the install directory.
- `config.toml`, `delays.toml`, `calibration.toml`, `plans/`, `logs/`, and the
  data cache are never replaced by the updater.
- `config.example.toml` and `data/pob-lock.json` are included in a fresh install
  ZIP, but are not copied over an existing user install.
- Debug builds check for releases but do not install them. Set
  `POE_CRAFTER_ALLOW_DEBUG_UPDATE=1` to exercise installs from a debug build.

The first release containing this updater must be installed manually. Later
releases can update themselves.

## Publishing a release

1. Bump `[workspace.package] version` in `Cargo.toml`.
2. Commit the version bump.
3. Tag and push:

   ```powershell
   git tag v0.1.1
   git push origin v0.1.1
   ```

The tag triggers `.github/workflows/release.yml`. The workflow:

1. Fails if the tag does not match the Cargo version.
2. Builds `poe-ui.exe` and `poe.exe` in release mode.
3. Creates a portable ZIP containing the binaries, `config.example.toml`,
   `README.md`, `plans/`, and `data/pob-lock.json`.
4. Uploads `poe-crafter-windows-x64.zip` to the GitHub release.

Use `v*` tags only for stable releases. The client uses GitHub's `latest`
endpoint, which excludes prereleases.

Do not change the asset name without updating `ASSET_NAME` in
`crates/poe-update/src/lib.rs`.
