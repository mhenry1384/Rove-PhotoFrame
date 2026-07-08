import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

type AppSettings = {
  directoryPath: string;
  intervalSeconds: number;
  foregroundNudgeEnabled: boolean;
  foregroundNudgeIntervalMinutes: number;
  imageMaximized: boolean;
  showPathOnPhoto: boolean;
  showDateOnPhoto: boolean;
};

type AppState = {
  currentImageIndex: number | null;
  imagePaths: string[];
  imageRequestToken: number;
  isImageMaximized: boolean;
  isPaused: boolean;
  settings: AppSettings;
  shuffledOrder: number[];
  shufflePosition: number;
  timerId: number | null;
};

const state: AppState = {
  currentImageIndex: null,
  imagePaths: [],
  imageRequestToken: 0,
  isImageMaximized: false,
  isPaused: false,
  settings: {
    directoryPath: "",
    intervalSeconds: 60,
    foregroundNudgeEnabled: true,
    foregroundNudgeIntervalMinutes: 1,
    imageMaximized: false,
    showPathOnPhoto: true,
    showDateOnPhoto: true,
  },
  shuffledOrder: [],
  shufflePosition: -1,
  timerId: null,
};

const elements = {
  appShell: document.querySelector<HTMLElement>(".app-shell"),
  currentImageName: document.querySelector<HTMLElement>("#current-image-name"),
  emptyMessage: document.querySelector<HTMLElement>("#empty-message"),
  imageCounter: document.querySelector<HTMLElement>("#image-counter"),
  imageDate: document.querySelector<HTMLElement>("#image-date"),
  imagePathEl: document.querySelector<HTMLElement>("#image-path"),
  intervalInput: document.querySelector<HTMLInputElement>("#interval-seconds"),
  browseButton: document.querySelector<HTMLButtonElement>("#browse-directory"),
  saveButton: document.querySelector<HTMLButtonElement>("#settings-save"),
  overlayDate: document.querySelector<HTMLElement>("#overlay-date"),
  overlayPath: document.querySelector<HTMLElement>("#overlay-path"),
  showPathOnPhotoInput: document.querySelector<HTMLInputElement>("#show-path-on-photo"),
  showDateOnPhotoInput: document.querySelector<HTMLInputElement>("#show-date-on-photo"),
  foregroundNudgeIntervalInput: document.querySelector<HTMLInputElement>(
    "#foreground-nudge-interval-minutes",
  ),
  nextOverlayButton: document.querySelector<HTMLButtonElement>("#next-image-overlay"),
  pauseResumeButton: document.querySelector<HTMLButtonElement>("#pause-resume-overlay"),
  prevOverlayButton: document.querySelector<HTMLButtonElement>("#prev-image-overlay"),
  photo: document.querySelector<HTMLImageElement>("#photo"),
  photoBg: document.querySelector<HTMLElement>("#photo-bg"),
  photoStage: document.querySelector<HTMLElement>("#photo-stage"),
  restoreLayoutButton: document.querySelector<HTMLButtonElement>("#restore-layout"),
  settingsButton: document.querySelector<HTMLButtonElement>("#open-settings"),
  settingsDialog: document.querySelector<HTMLDialogElement>("#settings-dialog"),
  settingsForm: document.querySelector<HTMLFormElement>("#settings-form"),
  settingsStatus: document.querySelector<HTMLElement>("#settings-status"),
  directoryInput: document.querySelector<HTMLInputElement>("#directory-path"),
  viewerStatus: document.querySelector<HTMLElement>("#viewer-status"),
};

function requireElement<T>(element: T | null, selector: string): T {
  if (!element) {
    throw new Error(`Missing element: ${selector}`);
  }

  return element;
}

const appShell = requireElement(elements.appShell, ".app-shell");
const currentImageName = requireElement(elements.currentImageName, "#current-image-name");
const emptyMessage = requireElement(elements.emptyMessage, "#empty-message");
const imageCounter = requireElement(elements.imageCounter, "#image-counter");
const imageDate = requireElement(elements.imageDate, "#image-date");
const imagePathEl = requireElement(elements.imagePathEl, "#image-path");
const intervalInput = requireElement(elements.intervalInput, "#interval-seconds");
const browseButton = requireElement(elements.browseButton, "#browse-directory");
const saveButton = requireElement(elements.saveButton, "#settings-save");
const overlayDate = requireElement(elements.overlayDate, "#overlay-date");
const overlayPath = requireElement(elements.overlayPath, "#overlay-path");
const showPathOnPhotoInput = requireElement(elements.showPathOnPhotoInput, "#show-path-on-photo");
const showDateOnPhotoInput = requireElement(elements.showDateOnPhotoInput, "#show-date-on-photo");
const foregroundNudgeIntervalInput = requireElement(
  elements.foregroundNudgeIntervalInput,
  "#foreground-nudge-interval-minutes",
);
const nextOverlayButton = requireElement(elements.nextOverlayButton, "#next-image-overlay");
const pauseResumeButton = requireElement(elements.pauseResumeButton, "#pause-resume-overlay");
const prevOverlayButton = requireElement(elements.prevOverlayButton, "#prev-image-overlay");
const photo = requireElement(elements.photo, "#photo");
const photoBg = requireElement(elements.photoBg, "#photo-bg");
const photoStage = requireElement(elements.photoStage, "#photo-stage");
const restoreLayoutButton = requireElement(elements.restoreLayoutButton, "#restore-layout");
const settingsButton = requireElement(elements.settingsButton, "#open-settings");
const settingsDialog = requireElement(elements.settingsDialog, "#settings-dialog");
const settingsForm = requireElement(elements.settingsForm, "#settings-form");
const settingsStatus = requireElement(elements.settingsStatus, "#settings-status");
const directoryInput = requireElement(elements.directoryInput, "#directory-path");
const viewerStatus = requireElement(elements.viewerStatus, "#viewer-status");

let foregroundNudgeTimerId: number | null = null;
let nativeCoverageListenerReady = false;

type WindowCoverageEvent = {
  covered: boolean;
};

function logForegroundNudgeTimer(action: "started" | "stopped" | "reset", detail?: string) {
  const suffix = detail ? ` (${detail})` : "";
  void invoke("debug_log", {
    message: `[foreground-nudge-timer] ${action}${suffix}`,
  });
}

function createForegroundNudgeInterval(reason?: string, action: "started" | "reset" = "started") {
  const intervalMs = state.settings.foregroundNudgeIntervalMinutes * 60 * 1000;
  foregroundNudgeTimerId = window.setInterval(() => {
    logForegroundNudgeTimer("reset", "interval elapsed");
    void invoke("raise_window_without_focus").catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      setViewerStatus(message, true);
      stopForegroundNudgeTimer("raise window failed");
    });
  }, intervalMs);

  logForegroundNudgeTimer(
    action,
    `${state.settings.foregroundNudgeIntervalMinutes} minute(s)${reason ? `, ${reason}` : ""}`,
  );
}

function stopForegroundNudgeTimer(reason?: string) {
  if (foregroundNudgeTimerId !== null) {
    window.clearInterval(foregroundNudgeTimerId);
    foregroundNudgeTimerId = null;
    logForegroundNudgeTimer("stopped", reason);
  }
}

function startForegroundNudgeTimer(reason?: string) {
  if (!state.settings.foregroundNudgeEnabled) {
    return;
  }

  if (document.hasFocus()) {
    return;
  }

  if (foregroundNudgeTimerId !== null) {
    return;
  }

  createForegroundNudgeInterval(reason, "started");
}

function startForegroundNudgeTimerOnFocusLoss(reason?: string) {
  if (!state.settings.foregroundNudgeEnabled) {
    return;
  }

  if (foregroundNudgeTimerId !== null) {
    resetForegroundNudgeTimer(reason);
    return;
  }

  createForegroundNudgeInterval(reason, "started");
}

function resetForegroundNudgeTimer(reason?: string) {
  if (foregroundNudgeTimerId === null) {
    return;
  }

  if (!state.settings.foregroundNudgeEnabled || document.hasFocus()) {
    return;
  }

  window.clearInterval(foregroundNudgeTimerId);
  foregroundNudgeTimerId = null;

  createForegroundNudgeInterval(reason, "reset");
}

function restartForegroundNudgeTimer() {
  if (!state.settings.foregroundNudgeEnabled) {
    stopForegroundNudgeTimer("feature disabled");
    return;
  }

  if (document.hasFocus()) {
    stopForegroundNudgeTimer("window focused");
    return;
  }

  if (foregroundNudgeTimerId !== null) {
    resetForegroundNudgeTimer("restart requested");
    return;
  }

  startForegroundNudgeTimer("restart requested");
}

function setViewerMessage(message: string) {
  emptyMessage.textContent = message;
  photoStage.dataset.empty = "true";
  photo.hidden = true;
  photo.alt = "";
}

function clearViewerMessage() {
  photoStage.dataset.empty = "false";
  photo.hidden = false;
}

function setSettingsStatus(message: string, isError = false) {
  settingsStatus.textContent = message;
  settingsStatus.dataset.error = isError ? "true" : "false";
}

function setViewerStatus(message: string, isError = false) {
  viewerStatus.textContent = message;
  viewerStatus.dataset.error = isError ? "true" : "false";
}

function syncImageMaximizedState() {
  appShell.dataset.imageMaximized = state.isImageMaximized ? "true" : "false";
  appShell.dataset.showPathOnPhoto = state.settings.showPathOnPhoto ? "true" : "false";
  appShell.dataset.showDateOnPhoto = state.settings.showDateOnPhoto ? "true" : "false";
  const buttonLabel = state.isImageMaximized
    ? "Exit max image mode"
    : "Enter max image mode";

  restoreLayoutButton.setAttribute("aria-label", buttonLabel);
  restoreLayoutButton.title = buttonLabel;
}

function setImageMaximized(isMaximized: boolean) {
  state.isImageMaximized = isMaximized;
  syncImageMaximizedState();
}

async function persistSettings() {
  const savedSettings = await invoke<AppSettings>("save_settings", {
    settings: state.settings,
  });

  state.settings = savedSettings;
}

function resetTimer() {
  if (state.timerId !== null) {
    window.clearInterval(state.timerId);
    state.timerId = null;
  }
}

function updateImageCounter() {
  imageCounter.textContent = String(state.imagePaths.length);
}

function buildShuffledOrder(length: number): number[] {
  const order = Array.from({ length }, (_, i) => i);
  for (let i = order.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [order[i], order[j]] = [order[j], order[i]];
  }
  return order;
}

function advanceShufflePosition(): number {
  const next = state.shufflePosition + 1;
  if (next >= state.shuffledOrder.length) {
    state.shuffledOrder = buildShuffledOrder(state.imagePaths.length);
    state.shufflePosition = 0;
  } else {
    state.shufflePosition = next;
  }
  return state.shuffledOrder[state.shufflePosition];
}

const PREFETCH_COUNT = 3;

// Peeks ahead in the shuffle order without mutating it. Stops at the order's end rather
// than simulating a reshuffle, since that reshuffle hasn't actually happened yet.
function peekUpcomingImagePaths(count: number): string[] {
  const upcoming: string[] = [];
  for (let i = 1; i <= count; i++) {
    const position = state.shufflePosition + i;
    if (position >= state.shuffledOrder.length) {
      break;
    }
    upcoming.push(state.imagePaths[state.shuffledOrder[position]]);
  }
  return upcoming;
}

function prefetchUpcomingImages() {
  const upcoming = peekUpcomingImagePaths(PREFETCH_COUNT);
  if (upcoming.length > 0) {
    void invoke("prefetch_images", { imagePaths: upcoming }).catch(() => {});
  }
}

async function renderCurrentImage(index: number) {
  const imagePath = state.imagePaths[index];
  const fileName = imagePath.split(/[/\\]/).pop() ?? imagePath;
  const requestToken = state.imageRequestToken + 1;

  state.imageRequestToken = requestToken;
  setViewerStatus(`Loading ${fileName}...`);

  const [imageSource, exif] = await Promise.all([
    invoke<string>("load_image_data_url", { imagePath }),
    invoke<{ date: string | null; description: string | null }>("get_image_exif", { imagePath }),
  ]);
  if (requestToken !== state.imageRequestToken) {
    return;
  }

  state.currentImageIndex = index;
  photo.src = imageSource;
  photoBg.style.backgroundImage = `url(${imageSource})`;
  photo.alt = fileName;
  currentImageName.textContent = fileName;
  const dir = state.settings.directoryPath.replace(/[/\\]+$/, "");
  const rel = state.imagePaths[index].slice(dir.length).replace(/^[/\\]/, "");
  const rawPath = exif.description ?? rel;
  const stripLast = (p: string) => { const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")); return i > 0 ? p.slice(0, i) : ""; };
  const overlayDisplayPath = stripLast(stripLast(rawPath));
  imagePathEl.textContent = exif.description ?? rel;
  imageDate.textContent = exif.date ?? "";
  overlayDate.textContent = exif.date ?? "";
  overlayPath.textContent = overlayDisplayPath;
  clearViewerMessage();
  updateImageCounter();
  setViewerStatus(`Showing ${fileName}`);
}

async function showRandomImage() {
  if (state.imagePaths.length === 0) {
    state.currentImageIndex = null;
    currentImageName.textContent = "No image selected";
    updateImageCounter();
    setViewerMessage("Choose a directory in Settings to begin the slideshow.");
    setViewerStatus("No images loaded.");
    return;
  }

  const nextIndex = advanceShufflePosition();

  try {
    await renderCurrentImage(nextIndex);
    prefetchUpcomingImages();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setViewerMessage("The selected image could not be displayed.");
    setViewerStatus(message, true);
  }
}

async function showPreviousImage() {
  if (state.imagePaths.length === 0 || state.shufflePosition <= 0) {
    return;
  }

  state.shufflePosition--;
  const index = state.shuffledOrder[state.shufflePosition];

  try {
    await renderCurrentImage(index);
    prefetchUpcomingImages();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setViewerMessage("The selected image could not be displayed.");
    setViewerStatus(message, true);
  }
}

function restartSlideshow() {
  resetTimer();

  if (state.isPaused || state.imagePaths.length === 0) {
    return;
  }

  state.timerId = window.setInterval(() => {
    void showRandomImage();
  }, state.settings.intervalSeconds * 1000);
}

function syncPauseState() {
  pauseResumeButton.dataset.paused = state.isPaused ? "true" : "false";
  const label = state.isPaused ? "Resume slideshow" : "Pause slideshow";
  pauseResumeButton.setAttribute("aria-label", label);
  pauseResumeButton.title = label;
}

function togglePause() {
  state.isPaused = !state.isPaused;
  syncPauseState();

  if (state.isPaused) {
    resetTimer();
    stopForegroundNudgeTimer("slideshow paused");
  } else {
    restartSlideshow();
    restartForegroundNudgeTimer();
  }
}

async function advanceToNextImage() {
  await showRandomImage();
  restartSlideshow();
}

async function refreshImages() {
  const directoryPath = state.settings.directoryPath.trim();

  if (!directoryPath) {
    state.imagePaths = [];
    state.currentImageIndex = null;
    currentImageName.textContent = "No directory configured";
    setViewerMessage("Set an image directory in Settings.");
    setViewerStatus("Waiting for configuration.");
    updateImageCounter();
    resetTimer();
    return;
  }

  const imagePaths = await invoke<string[]>("scan_images", { directoryPath });
  state.imagePaths = imagePaths;
  state.currentImageIndex = null;
  state.shuffledOrder = buildShuffledOrder(imagePaths.length);
  state.shufflePosition = -1;

  if (imagePaths.length === 0) {
    currentImageName.textContent = "No supported images found";
    setViewerMessage("No .jpg, .jpeg, .gif, .png, .webp, .heic, or .heif files were found in the selected directory.");
    setViewerStatus("Directory scanned, but it contained no supported images.");
    updateImageCounter();
    resetTimer();
    return;
  }

  await advanceToNextImage();
}

const INTERVAL_MINUTES = [1, 5, 15, 30, 60] as const;
const NUDGE_MINUTES = [0, 1, 5, 15, 60] as const;

function intervalSliderPosition(): number {
  const minutes = Math.round(state.settings.intervalSeconds / 60);
  const idx = (INTERVAL_MINUTES as readonly number[]).indexOf(minutes);
  return idx >= 0 ? idx : 2;
}

function nudgeSliderPosition(): number {
  if (!state.settings.foregroundNudgeEnabled) return 0;
  const idx = NUDGE_MINUTES.indexOf(state.settings.foregroundNudgeIntervalMinutes as typeof NUDGE_MINUTES[number]);
  return idx > 0 ? idx : 1;
}

function getFormSnapshot(): string {
  return JSON.stringify({
    dir: directoryInput.value.trim(),
    interval: intervalInput.value,
    nudge: foregroundNudgeIntervalInput.value,
    pathOnPhoto: showPathOnPhotoInput.checked,
    dateOnPhoto: showDateOnPhotoInput.checked,
  });
}

let settingsOpenSnapshot = "";

function syncFormFromState() {
  directoryInput.value = state.settings.directoryPath;
  intervalInput.value = String(intervalSliderPosition());
  foregroundNudgeIntervalInput.value = String(nudgeSliderPosition());
  showPathOnPhotoInput.checked = state.settings.showPathOnPhoto;
  showDateOnPhotoInput.checked = state.settings.showDateOnPhoto;
}

async function initializeApp() {
  try {
    state.settings = await invoke<AppSettings>("load_settings");
    setImageMaximized(state.settings.imageMaximized);
    syncFormFromState();
    await refreshImages();
    restartForegroundNudgeTimer();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setViewerMessage("The application settings could not be loaded.");
    setViewerStatus(message, true);
    setSettingsStatus(message, true);
  }
}

function setupDomCoverageFallbackListeners() {
  window.addEventListener("blur", () => {
    startForegroundNudgeTimerOnFocusLoss("DOM blur fallback");
  });

  window.addEventListener("focus", () => {
    stopForegroundNudgeTimer("DOM focus fallback");
  });

}

async function initializeNativeCoverageListener() {
  try {
    await listen<WindowCoverageEvent>("photoframe://window-coverage", (event) => {
      if (event.payload.covered) {
        // Native focus loss signal: start or reset countdown.
        startForegroundNudgeTimerOnFocusLoss("native focus lost");
        return;
      }

      stopForegroundNudgeTimer("native focus gained");
    });

    nativeCoverageListenerReady = true;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setViewerStatus(
      `Native window coverage events unavailable (${message}); using browser fallback.`,
    );
  }

  if (!nativeCoverageListenerReady) {
    setupDomCoverageFallbackListeners();
  }
}

browseButton.addEventListener("click", () => {
  const currentDir = directoryInput.value.trim() || undefined;
  void openDialog({ directory: true, multiple: false, defaultPath: currentDir }).then((selected) => {
    if (typeof selected === "string") {
      directoryInput.value = selected;
      directoryInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
});

settingsButton.addEventListener("click", () => {
  syncFormFromState();
  settingsOpenSnapshot = getFormSnapshot();
  saveButton.disabled = true;
  setSettingsStatus("");
  settingsDialog.showModal();
});

settingsForm.addEventListener("input", () => {
  saveButton.disabled = getFormSnapshot() === settingsOpenSnapshot;
});

settingsDialog.addEventListener("close", () => {
  setSettingsStatus("");
});

pauseResumeButton.addEventListener("click", () => {
  togglePause();
});

prevOverlayButton.addEventListener("click", () => {
  void showPreviousImage();
});

nextOverlayButton.addEventListener("click", () => {
  void advanceToNextImage();
});

restoreLayoutButton.addEventListener("click", () => {
  const nextImageMaximized = !state.isImageMaximized;
  setImageMaximized(nextImageMaximized);
  state.settings.imageMaximized = nextImageMaximized;

  void persistSettings().catch((error) => {
    const message = error instanceof Error ? error.message : String(error);
    setViewerStatus(message, true);
  });
});

photo.addEventListener("error", () => {
  setViewerMessage("The current image failed to load.");
  setViewerStatus("Image decoding failed in the webview.", true);
});

settingsForm.addEventListener("submit", async (event) => {
  event.preventDefault();

  const intervalPosition = Number.parseInt(intervalInput.value, 10);
  const intervalSeconds = (INTERVAL_MINUTES[intervalPosition] ?? 1) * 60;
  const nudgePosition = Number.parseInt(foregroundNudgeIntervalInput.value, 10);
  const nudgeMinutes = NUDGE_MINUTES[nudgePosition] ?? 0;

  try {
    state.settings.directoryPath = directoryInput.value.trim();
    state.settings.intervalSeconds = intervalSeconds;
    state.settings.foregroundNudgeEnabled = nudgeMinutes > 0;
    if (nudgeMinutes > 0) {
      state.settings.foregroundNudgeIntervalMinutes = nudgeMinutes;
    }
    state.settings.showPathOnPhoto = showPathOnPhotoInput.checked;
    state.settings.showDateOnPhoto = showDateOnPhotoInput.checked;
    await persistSettings();
    syncImageMaximizedState();
    syncFormFromState();
    settingsOpenSnapshot = getFormSnapshot();
    saveButton.disabled = true;
    restartForegroundNudgeTimer();
    await refreshImages();
    settingsDialog.close();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setSettingsStatus(message, true);
  }
});

window.addEventListener("DOMContentLoaded", () => {
  void initializeNativeCoverageListener();
  void initializeApp();
});
