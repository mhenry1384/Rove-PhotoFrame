# PhotoFrame

A desktop slideshow app built with [Tauri](https://tauri.app) and TypeScript. Point it at a folder of photos and it cycles through them automatically, staying visible on your desktop while you work.

![PhotoFrame screenshot](screenshot.png)

## Features

### Slideshow
- Shuffled playback — every image is shown once before any repeats
- Recursive subfolder scanning (`.jpg`, `.jpeg`, `.gif`, `.png`, `.webp`, `.heic`, `.heif`)
- Configurable interval: 1, 5, 15, 30, or 60 minutes
- Previous / pause / next controls
- Blurred background fill behind each photo

### Display
- **Maximized mode** — expands the photo to fill the full window, hiding the status bar
- **Path overlay** — shows the subfolder path at the bottom-left corner (maximized mode only)
- **Date overlay** — reads the EXIF `DateTimeOriginal` tag and displays the date (JPEG only, maximized mode only)

### HEIC/HEIF support
WebView2 can't render HEIC/HEIF natively, so those files are decoded on the Rust side via the Windows Imaging Component and re-encoded as JPEG — this requires the OS to have the "HEIF Image Extensions" codec installed (present by default on most Windows 10/11 installs, but not guaranteed). Converted JPEGs are cached in a per-process temp directory, keyed by a hash of the original path, so a photo is never reconverted twice in one run. After each photo is shown, the next few upcoming photos in the shuffle order are pre-converted in the background so they're ready by the time the slideshow reaches them.

### Foreground nudge
When the app is unfocused, it can periodically raise itself to the foreground without stealing keyboard focus. Configurable interval: Never, 1, 5, 15, or 60 minutes.

### Settings
- Image directory — freeform text or native folder picker
- All settings are persisted to `settings.json` in the platform app-config directory
- Window size and position are restored on launch

## Getting started

### Prerequisites

- [Node.js](https://nodejs.org) 18+
- [Rust](https://rustup.rs) (stable)
- [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform

### Install dependencies

```bash
npm install
```

### Run in development

```bash
npm run tauri dev
```

### Build for production

```bash
npm run tauri build
```

## Usage

1. Click **Settings** and choose an image directory (type a path or use the folder picker).
2. Set how often photos should change and how often the app should nudge itself to the foreground.
3. Click **Save**. The slideshow starts immediately.
4. Use the **◀ ⏸ ▶** buttons on the photo to navigate or pause.
5. Click the expand icon to enter maximized mode, which hides the UI and shows path/date overlays directly on the photo.

## Data stored

| File | Location | Contents |
|---|---|---|
| `settings.json` | Platform app-config dir | Directory, interval, nudge settings, overlay preferences |
| `window-state.json` | Platform app-config dir | Window position and size |
| Converted HEIC/HEIF JPEGs | `%TEMP%\photoframe_<pid>\` | Cached JPEG conversions, cleared on app close |

## Built with

- [Tauri 2](https://tauri.app) — native shell
- [Vite](https://vitejs.dev) + TypeScript — frontend
- [kamadak-exif](https://crates.io/crates/kamadak-exif) — EXIF date reading
- [image](https://crates.io/crates/image) + [windows](https://crates.io/crates/windows) — HEIC/HEIF decoding via Windows Imaging Component
