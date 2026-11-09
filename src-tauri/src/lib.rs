use exif;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    sync::{mpsc, Mutex, OnceLock},
};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, Window,
    WindowEvent,
};

static WINDOW_SAVE_ENABLED: AtomicBool = AtomicBool::new(false);

static TEMP_DIR: OnceLock<PathBuf> = OnceLock::new();

// Single background prefetch worker — all conversion requests are serialized through
// one thread to avoid concurrent WIC usage crashing the process.
static PREFETCH_TX: OnceLock<Mutex<mpsc::Sender<String>>> = OnceLock::new();

// Max pixel dimension for converted HEIC — keeps decoding fast on large sensor photos.
#[cfg(windows)]
const HEIC_MAX_DIM: u32 = 2048;

const SETTINGS_FILE_NAME: &str = "settings.json";
const WINDOW_STATE_FILE_NAME: &str = "window-state.json";
const FAVORITES_FILE_NAME: &str = "favorites.json";
const WINDOW_COVERAGE_EVENT_NAME: &str = "photoframe://window-coverage";
const DEFAULT_INTERVAL_SECONDS: u32 = 60;

fn default_foreground_nudge_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    directory_path: String,
    interval_seconds: u32,
    #[serde(default = "default_foreground_nudge_enabled")]
    foreground_nudge_enabled: bool,
    #[serde(default)]
    image_maximized: bool,
    #[serde(default = "default_true")]
    show_path_on_photo: bool,
    #[serde(default = "default_true")]
    show_date_on_photo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WindowState {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    maximized: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowCoverageEvent {
    covered: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            directory_path: String::new(),
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            foreground_nudge_enabled: true,
            image_maximized: false,
            show_path_on_photo: true,
            show_date_on_photo: true,
        }
    }
}

fn init_prefetch_worker() {
    PREFETCH_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for file_path in rx {
                if is_heic_ext(Path::new(&file_path)) {
                    let _ = ensure_heic_cached(Path::new(&file_path));
                }
            }
        });
        Mutex::new(tx)
    });
}

fn get_temp_dir() -> &'static PathBuf {
    TEMP_DIR.get_or_init(|| {
        let mut dir = std::env::temp_dir();
        dir.push(format!("photoframe_{}", std::process::id()));
        dir
    })
}

fn ensure_temp_dir() -> Result<&'static PathBuf, String> {
    let dir = get_temp_dir();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn temp_path_for(original: &str) -> Result<String, String> {
    let dir = ensure_temp_dir()?;
    let mut hasher = DefaultHasher::new();
    original.hash(&mut hasher);
    let hash = hasher.finish();
    Ok(dir
        .join(format!("{:016x}.jpg", hash))
        .to_string_lossy()
        .into_owned())
}

fn is_heic_ext(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("heic" | "heif")
    )
}

// Ensures a converted-JPEG cache exists for the given HEIC/HEIF file and returns its path.
fn ensure_heic_cached(path: &Path) -> Result<PathBuf, String> {
    let original = path.to_string_lossy().to_string();
    let temp = temp_path_for(&original)?;
    let temp_path = PathBuf::from(&temp);
    if !temp_path.exists() {
        convert_heic_to_jpeg(&original, &temp)?;
    }
    Ok(temp_path)
}

#[cfg(windows)]
fn convert_heic_to_jpeg(heic_path: &str, jpeg_path: &str) -> Result<(), String> {
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::GENERIC_READ,
            Graphics::Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat24bppBGR, IWICBitmapScaler,
                IWICFormatConverter, IWICImagingFactory, WICBitmapDitherTypeNone,
                WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
                WICDecodeMetadataCacheOnDemand, WICRect,
            },
            System::Com::{
                CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
            },
        },
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("WIC init failed: {e}"))?;

        let path_wide: Vec<u16> = heic_path.encode_utf16().chain(Some(0)).collect();
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR::from_raw(path_wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| format!("HEIC decode failed (HEIF Image Extensions required): {e}"))?;

        let frame = decoder
            .GetFrame(0)
            .map_err(|e| format!("Frame read failed: {e}"))?;

        // Scale down large photos to HEIC_MAX_DIM before pixel copy — dramatically
        // reduces memory and encode time for high-resolution sensor images.
        let mut orig_w = 0u32;
        let mut orig_h = 0u32;
        frame
            .GetSize(&mut orig_w, &mut orig_h)
            .map_err(|e| format!("GetSize failed: {e}"))?;

        let scale = (HEIC_MAX_DIM as f64 / orig_w.max(orig_h) as f64).min(1.0);
        let out_w = ((orig_w as f64 * scale).round() as u32).max(1);
        let out_h = ((orig_h as f64 * scale).round() as u32).max(1);

        let scaler: IWICBitmapScaler = factory
            .CreateBitmapScaler()
            .map_err(|e| format!("Scaler create failed: {e}"))?;
        scaler
            .Initialize(&frame, out_w, out_h, WICBitmapInterpolationModeFant)
            .map_err(|e| format!("Scaler init failed: {e}"))?;

        let converter: IWICFormatConverter = factory
            .CreateFormatConverter()
            .map_err(|e| format!("Format converter failed: {e}"))?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat24bppBGR,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )
            .map_err(|e| format!("Converter init failed: {e}"))?;

        let stride = out_w * 3;
        let mut buffer = vec![0u8; (stride * out_h) as usize];

        let rect = WICRect {
            X: 0,
            Y: 0,
            Width: out_w as i32,
            Height: out_h as i32,
        };
        converter
            .CopyPixels(&rect, stride, &mut buffer)
            .map_err(|e| format!("CopyPixels failed: {e}"))?;

        // WIC returns BGR; image crate expects RGB
        for chunk in buffer.chunks_exact_mut(3) {
            chunk.swap(0, 2);
        }

        let img = image::RgbImage::from_raw(out_w, out_h, buffer)
            .ok_or("Failed to construct image from pixel data")?;
        img.save_with_format(jpeg_path, image::ImageFormat::Jpeg)
            .map_err(|e| format!("JPEG save failed: {e}"))?;

        Ok(())
    }
}

#[cfg(not(windows))]
fn convert_heic_to_jpeg(_heic_path: &str, _jpeg_path: &str) -> Result<(), String> {
    Err("HEIC and HEIF images are currently supported only on Windows.".into())
}

#[tauri::command]
fn prefetch_images(image_paths: Vec<String>) {
    init_prefetch_worker();
    if let Some(tx) = PREFETCH_TX.get() {
        if let Ok(sender) = tx.lock() {
            for path in image_paths {
                let _ = sender.send(path);
            }
        }
    }
}

fn cleanup_temp_dir() {
    if let Some(dir) = TEMP_DIR.get() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                fs::remove_file(entry.path()).ok();
            }
        }
    }
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<AppSettings, String> {
    let settings_path = app_file_path(&app, SETTINGS_FILE_NAME)?;

    if !settings_path.exists() {
        let defaults = AppSettings::default();
        write_json_file(&settings_path, &defaults)?;
        return Ok(defaults);
    }

    let settings: AppSettings = read_json_file(&settings_path)?;
    validate_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: AppSettings) -> Result<AppSettings, String> {
    validate_settings(&settings)?;

    let settings_path = app_file_path(&app, SETTINGS_FILE_NAME)?;
    write_json_file(&settings_path, &settings)?;

    Ok(settings)
}

#[tauri::command]
fn raise_window_without_focus(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is not available.".to_string())?;

    window
        .show()
        .map_err(|error| format!("Failed to show window: {error}"))?;

    window
        .set_always_on_top(true)
        .map_err(|error| format!("Failed to raise window: {error}"))?;

    let window_clone = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(120));
        let _ = window_clone.set_always_on_top(false);
    });

    Ok(())
}

#[tauri::command]
fn debug_log(message: String) {
    println!("[Rove PhotoFrame] {message}");
}

// Async + spawn_blocking so a long scan of a large tree runs off the main thread and
// doesn't freeze the webview (sync commands run on the main thread).
#[tauri::command]
async fn scan_images(directory_path: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || scan_images_blocking(&directory_path))
        .await
        .map_err(|error| format!("Directory scan failed: {error}"))?
}

fn scan_images_blocking(directory_path: &str) -> Result<Vec<String>, String> {
    let directory = PathBuf::from(directory_path.trim());

    if !directory.exists() {
        return Err("The configured directory does not exist.".into());
    }

    if !directory.is_dir() {
        return Err("The configured path is not a directory.".into());
    }

    let mut images = Vec::new();
    let mut dirs = std::collections::VecDeque::new();
    dirs.push_back(directory);

    while let Some(dir) = dirs.pop_front() {
        let entries =
            fs::read_dir(&dir).map_err(|error| format!("Failed to read directory: {error}"))?;

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                dirs.push_back(path);
            } else if is_supported_image(&path) {
                images.push(path.to_string_lossy().to_string());
            }
        }
    }

    images.sort_unstable();
    Ok(images)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageExif {
    date: Option<String>,
    description: Option<String>,
}

fn format_exif_date(exif: &exif::Exif) -> Option<String> {
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let dt = if let exif::Value::Ascii(ref vec) = field.value {
        vec.first()
            .and_then(|b| exif::DateTime::from_ascii(b).ok())?
    } else {
        return None;
    };
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let month = months.get(dt.month as usize - 1)?;
    Some(format!("{month} {}, {}", dt.day, dt.year))
}

fn read_exif_description(exif: &exif::Exif) -> Option<String> {
    let field = exif.get_field(exif::Tag::ImageDescription, exif::In::PRIMARY)?;
    if let exif::Value::Ascii(ref vec) = field.value {
        let desc = String::from_utf8_lossy(vec.first()?).trim().to_string();
        if !desc.is_empty() {
            return Some(desc);
        }
    }
    None
}

fn read_gif_comment(path: &Path) -> Option<String> {
    let data = fs::read(path).ok()?;
    if data.len() < 13 {
        return None;
    }
    if &data[0..6] != b"GIF89a" && &data[0..6] != b"GIF87a" {
        return None;
    }
    let packed = data[10];
    let gct_size = if packed & 0x80 != 0 {
        3 * (1usize << ((packed & 0x07) as usize + 1))
    } else {
        0
    };
    let mut pos = 13 + gct_size;

    while pos + 1 < data.len() {
        match data[pos] {
            0x3B => break, // trailer
            0x2C => break, // image descriptor — comments come before images; stop here
            0x21 => {
                let label = data[pos + 1];
                pos += 2;
                let is_comment = label == 0xFE;
                let mut block_data = Vec::new();
                while pos < data.len() {
                    let block_len = data[pos] as usize;
                    pos += 1;
                    if block_len == 0 {
                        break;
                    }
                    if pos + block_len > data.len() {
                        break;
                    }
                    if is_comment {
                        block_data.extend_from_slice(&data[pos..pos + block_len]);
                    }
                    pos += block_len;
                }
                if is_comment {
                    let s = String::from_utf8_lossy(&block_data).trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            _ => break,
        }
    }
    None
}

#[tauri::command]
fn get_image_exif(image_path: String) -> ImageExif {
    let mut result = ImageExif {
        date: None,
        description: None,
    };

    let path = PathBuf::from(image_path.trim());
    let ext = match path.extension() {
        Some(e) => e.to_string_lossy().to_lowercase(),
        None => return result,
    };

    if ext == "gif" {
        result.description = read_gif_comment(&path);
        return result;
    }

    if ext == "jpg" || ext == "jpeg" || ext == "png" || ext == "heic" || ext == "heif" {
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return result,
        };
        let mut reader = std::io::BufReader::new(file);
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
            result.date = format_exif_date(&exif);
            result.description = read_exif_description(&exif);
        }
    }

    result
}

#[tauri::command]
fn load_image_data_url(image_path: String) -> Result<String, String> {
    let path = PathBuf::from(image_path.trim());

    if !path.exists() {
        return Err("The requested image does not exist.".into());
    }

    if !path.is_file() {
        return Err("The requested image path is not a file.".into());
    }

    if !is_supported_image(&path) {
        return Err("The requested image format is not supported.".into());
    }

    let mime_type = image_mime_type(&path)?;
    let read_path = if is_heic_ext(&path) {
        ensure_heic_cached(&path)?
    } else {
        path.clone()
    };

    let bytes = fs::read(&read_path)
        .map_err(|error| format!("Failed to read image {}: {error}", read_path.display()))?;
    let encoded = {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        STANDARD.encode(bytes)
    };

    Ok(format!("data:{mime_type};base64,{encoded}"))
}

#[tauri::command]
fn copy_image_to_path(image_path: String, destination_path: String) -> Result<(), String> {
    let source = PathBuf::from(image_path.trim());
    let destination = PathBuf::from(destination_path.trim());

    if !source.is_file() || !is_supported_image(&source) {
        return Err("The requested image is not available for download.".into());
    }

    if destination.as_os_str().is_empty() {
        return Err("Choose a destination for the image.".into());
    }

    fs::copy(&source, &destination)
        .map_err(|error| format!("Failed to save image to {}: {error}", destination.display()))?;

    Ok(())
}

#[tauri::command]
fn copy_text_to_clipboard(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|error| format!("Failed to copy text to the clipboard: {error}"))
}

#[tauri::command]
fn copy_image_to_clipboard(image_path: String) -> Result<(), String> {
    let source = PathBuf::from(image_path.trim());

    if !source.is_file() || !is_supported_image(&source) {
        return Err("The requested image is not available for copying.".into());
    }

    let read_path = if is_heic_ext(&source) {
        ensure_heic_cached(&source)?
    } else {
        source
    };
    let image = image::ImageReader::open(&read_path)
        .map_err(|error| format!("Failed to open image {}: {error}", read_path.display()))?
        .decode()
        .map_err(|error| format!("Failed to decode image {}: {error}", read_path.display()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Failed to access the clipboard: {error}"))?;

    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
        .map_err(|error| format!("Failed to copy image to the clipboard: {error}"))
}

// Favorites live in favorites.json in the per-user config dir. Every change is written
// to disk immediately, so nothing is lost when the app closes.
fn read_favorites(app: &AppHandle) -> Result<Vec<String>, String> {
    let path = app_file_path(app, FAVORITES_FILE_NAME)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    read_json_file(&path)
}

fn write_favorites(app: &AppHandle, favorites: &[String]) -> Result<(), String> {
    let path = app_file_path(app, FAVORITES_FILE_NAME)?;
    write_json_file(&path, &favorites)
}

#[tauri::command]
fn load_favorites(app: AppHandle) -> Result<Vec<String>, String> {
    read_favorites(&app)
}

// Adds or removes the image from favorites; returns whether it is now a favorite.
#[tauri::command]
fn set_favorite(app: AppHandle, image_path: String, favorite: bool) -> Result<bool, String> {
    let mut favorites = read_favorites(&app)?;
    favorites.retain(|existing| existing != &image_path);
    if favorite {
        favorites.push(image_path);
    }
    write_favorites(&app, &favorites)?;
    Ok(favorite)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportFavoritesResult {
    copied: usize,
    failed: Vec<String>,
    remaining: Vec<String>,
}

// Picks a file name in `directory` that doesn't already exist, appending " (n)" if needed.
fn unique_destination(directory: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    let candidate = directory.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let name = Path::new(file_name);
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    (2..)
        .map(|n| directory.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .expect("an unused file name always exists")
}

// Copies every favorite into the destination folder and un-favorites the ones that
// were copied. Favorites that fail to copy stay favorited so nothing is lost.
#[tauri::command]
async fn export_favorites(
    app: AppHandle,
    destination_dir: String,
) -> Result<ExportFavoritesResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let destination = PathBuf::from(destination_dir.trim());
        if destination.as_os_str().is_empty() {
            return Err("Choose a destination folder.".to_string());
        }
        if !destination.is_dir() {
            return Err("The destination folder does not exist.".to_string());
        }

        let favorites = read_favorites(&app)?;
        let mut copied = 0;
        let mut failed = Vec::new();
        let mut remaining = Vec::new();

        for favorite in favorites {
            let source = PathBuf::from(&favorite);
            let result = match source.file_name() {
                Some(file_name) if source.is_file() => {
                    let target = unique_destination(&destination, file_name);
                    fs::copy(&source, &target)
                        .map(|_| ())
                        .map_err(|error| format!("{favorite}: {error}"))
                }
                _ => Err(format!("{favorite}: file not found")),
            };

            match result {
                Ok(()) => copied += 1,
                Err(message) => {
                    failed.push(message);
                    remaining.push(favorite);
                }
            }
        }

        write_favorites(&app, &remaining)?;
        Ok(ExportFavoritesResult {
            copied,
            failed,
            remaining,
        })
    })
    .await
    .map_err(|error| format!("Copying favorites failed: {error}"))?
}

fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if settings.interval_seconds == 0 {
        return Err("Interval must be at least 1 second.".into());
    }

    let directory_path = settings.directory_path.trim();
    if directory_path.is_empty() {
        return Ok(());
    }

    let directory = Path::new(directory_path);
    if !directory.exists() {
        return Err("The configured directory does not exist.".into());
    }

    if !directory.is_dir() {
        return Err("The configured path is not a directory.".into());
    }

    Ok(())
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "gif" | "png" | "webp" | "heic" | "heif")
    )
}

fn image_mime_type(path: &Path) -> Result<&'static str, String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => Ok("image/jpeg"),
        Some("gif") => Ok("image/gif"),
        Some("png") => Ok("image/png"),
        Some("webp") => Ok("image/webp"),
        Some("heic" | "heif") => Ok("image/jpeg"), // served from the converted-JPEG cache
        _ => Err("The requested image format is not supported.".into()),
    }
}

fn app_file_path(app: &AppHandle, file_name: &str) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("Failed to resolve config directory: {error}"))?;

    fs::create_dir_all(&config_dir)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;

    Ok(config_dir.join(file_name))
}

fn read_json_file<T>(path: &Path) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;

    serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse {}: {error}", path.display()))
}

fn write_json_file<T>(path: &Path, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let contents = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize {}: {error}", path.display()))?;

    fs::write(path, format!("{contents}\n"))
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn restore_window_state(window: &Window) -> Result<(), String> {
    let state_path = app_file_path(&window.app_handle(), WINDOW_STATE_FILE_NAME)?;
    if !state_path.exists() {
        return Ok(());
    }

    let state: WindowState = read_json_file(&state_path)?;
    if state.width <= 100.0 || state.height <= 100.0 {
        return Ok(());
    }

    // Reject the position if no monitor contains the window's top-left corner,
    // which catches stale off-screen state (e.g. a disconnected monitor) without
    // incorrectly rejecting legitimate negative X positions on left-side monitors.
    let monitors = window.available_monitors().unwrap_or_default();
    let monitor = monitors.iter().find(|m| {
        let sf = m.scale_factor();
        let pos = m.position().to_logical::<f64>(sf);
        let size = m.size().to_logical::<f64>(sf);
        state.x >= pos.x
            && state.x < pos.x + size.width
            && state.y >= pos.y
            && state.y < pos.y + size.height
    });
    let Some(monitor) = monitor else {
        // A maximized window's own bounds overhang its monitor, so without an
        // earlier normal position it can only be maximized where it opened.
        if state.maximized {
            window
                .maximize()
                .map_err(|error| format!("Failed to maximize window: {error}"))?;
        }
        return Ok(());
    };

    // The state was saved in the target monitor's logical units, so convert with
    // its scale factor rather than that of the monitor the window opened on.
    let scale_factor = monitor.scale_factor();
    let x = (state.x * scale_factor).round() as i32;
    let y = (state.y * scale_factor).round() as i32;
    let width = (state.width * scale_factor).round() as i32;
    let height = (state.height * scale_factor).round() as i32;

    // set_size applies to the client area, so the title bar and borders have to
    // be included when checking that the window fits on the monitor.
    let inner = window
        .inner_size()
        .map_err(|error| format!("Failed to read window size: {error}"))?;
    let outer = window
        .outer_size()
        .map_err(|error| format!("Failed to read window size: {error}"))?;
    let frame_width = outer.width as i32 - inner.width as i32;
    let frame_height = outer.height as i32 - inner.height as i32;

    let work_area = monitor.work_area();
    let max_width = work_area.position.x + work_area.size.width as i32 - x - frame_width;
    let max_height = work_area.position.y + work_area.size.height as i32 - y - frame_height;

    window
        .set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|error| format!("Failed to restore window position: {error}"))?;

    window
        .set_size(Size::Physical(PhysicalSize::new(
            width.min(max_width).max(1) as u32,
            height.min(max_height).max(1) as u32,
        )))
        .map_err(|error| format!("Failed to restore window size: {error}"))?;

    if state.maximized {
        window
            .maximize()
            .map_err(|error| format!("Failed to maximize window: {error}"))?;
    }

    Ok(())
}

fn fills_monitor(window: &Window) -> Result<bool, String> {
    let Some(monitor) = window
        .current_monitor()
        .map_err(|error| format!("Failed to read current monitor: {error}"))?
    else {
        return Ok(false);
    };

    let position = window
        .outer_position()
        .map_err(|error| format!("Failed to read window position: {error}"))?;
    let size = window
        .outer_size()
        .map_err(|error| format!("Failed to read window size: {error}"))?;
    let area = monitor.work_area();

    Ok(position.x <= area.position.x
        && position.y <= area.position.y
        && position.x + size.width as i32 >= area.position.x + area.size.width as i32
        && position.y + size.height as i32 >= area.position.y + area.size.height as i32)
}

fn save_window_state(window: &Window) -> Result<(), String> {
    if window.is_minimized().unwrap_or(false) {
        return Ok(());
    }

    // Save the client area, because that is what set_size restores.
    let physical_size = window
        .inner_size()
        .map_err(|error| format!("Failed to read window size: {error}"))?;
    let physical_position = window
        .outer_position()
        .map_err(|error| format!("Failed to read window position: {error}"))?;
    let scale_factor = window
        .scale_factor()
        .map_err(|error| format!("Failed to read window scale factor: {error}"))?;

    let size = physical_size.to_logical::<f64>(scale_factor);
    let position = physical_position.to_logical::<f64>(scale_factor);

    if size.width <= 0.0 || size.height <= 0.0 {
        return Ok(());
    }

    let maximized = window.is_maximized().unwrap_or(false);
    let state_path = app_file_path(&window.app_handle(), WINDOW_STATE_FILE_NAME)?;

    // While maximized, keep the last normal bounds so un-maximizing after a
    // restart returns the window to the size it had before. The bounds are also
    // checked because Windows reports the maximized size before is_maximized
    // turns true, which would otherwise overwrite the normal bounds.
    let keep_previous_bounds = maximized || fills_monitor(window)?;
    let previous = keep_previous_bounds
        .then(|| read_json_file::<WindowState>(&state_path).ok())
        .flatten();

    let state = match previous {
        Some(previous) => WindowState {
            maximized,
            ..previous
        },
        None => WindowState {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized,
        },
    };

    write_json_file(&state_path, &state)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_prefetch_worker();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_handle = app.handle().clone();

            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Some(window) = app_handle.get_webview_window("main") {
                    let host_window = window.as_ref().window();
                    let _ = restore_window_state(&host_window);
                    let _ = host_window.show();
                }

                WINDOW_SAVE_ENABLED.store(true, Ordering::Release);
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(is_focused) = event {
                let _ = window.emit(
                    WINDOW_COVERAGE_EVENT_NAME,
                    WindowCoverageEvent {
                        covered: !is_focused,
                    },
                );
            }

            if matches!(
                event,
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
            ) {
                cleanup_temp_dir();
            }

            if matches!(
                event,
                WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
            ) || (WINDOW_SAVE_ENABLED.load(Ordering::Acquire)
                && matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)))
            {
                let _ = save_window_state(window);
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            raise_window_without_focus,
            debug_log,
            scan_images,
            get_image_exif,
            load_image_data_url,
            copy_image_to_path,
            copy_image_to_clipboard,
            copy_text_to_clipboard,
            prefetch_images,
            load_favorites,
            set_favorite,
            export_favorites
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
