# AGENTS

## Build Versioning Policy

- Before running a release build (for example `npm run tauri build`), check `git status`. If there are uncommitted changes, increment the app version by `0.0.1`. If the working tree is clean, build without changing the version.
- Keep all version references in sync:
  - `package.json` (`version`)
  - `src-tauri/tauri.conf.json` (`version`)
  - `src-tauri/Cargo.toml` (`package.version`)
  - `index.html` displayed app version label
- Apply the version bump first, then run the build.
- When running a release build, check whether the app (`photoframe.exe`) is running. If it is, kill it before building and restart it from `src-tauri/target/release/photoframe.exe` after the build finishes, so the running app is always the latest build.

## Installers

- Never build an installer unless the user explicitly asks for one.
- `npm run tauri build` creates installers by default, because bundling is turned on in `src-tauri/tauri.conf.json`. For a release build without an installer, run `npm run tauri build -- --no-bundle`.
