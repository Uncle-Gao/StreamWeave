import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion, setTheme } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";

import "./styles.css";

document.documentElement.classList.add("app-ready");

const urlInput = document.querySelector("#url-input");
const filenameInput = document.querySelector("#filename-input");
const startButton = document.querySelector("#start-button");
const appVersionLabel = document.querySelector("#app-version");
const settingsAppVersionLabel = document.querySelector("#settings-app-version");
const activeSummary = document.querySelector("#active-summary");
const taskList = document.querySelector("#task-list");
const selectAllTasksInput = document.querySelector("#select-all-tasks");
const selectedSummary = document.querySelector("#selected-summary");
const batchPauseButton = document.querySelector("#batch-pause-button");
const batchResumeButton = document.querySelector("#batch-resume-button");
const batchCancelButton = document.querySelector("#batch-cancel-button");
const batchRetryButton = document.querySelector("#batch-retry-button");
const batchDeleteButton = document.querySelector("#batch-delete-button");
const openButton = document.querySelector("#open-button");
const revealButton = document.querySelector("#reveal-button");
const copyLogButton = document.querySelector("#copy-log-button");
const detailTitle = document.querySelector("#detail-title");
const detailSubtitle = document.querySelector("#detail-subtitle");
const stageLabel = document.querySelector("#stage-label");
const segmentLabel = document.querySelector("#segment-label");
const progress = document.querySelector("#progress");
const summary = document.querySelector("#summary");
const taskDirectory = document.querySelector("#task-directory");
const logOutput = document.querySelector("#log-output");
const duplicateDialog = document.querySelector("#duplicate-dialog");
const duplicateDialogMessage = document.querySelector("#duplicate-dialog-message");
const duplicateOpenButton = document.querySelector("#duplicate-open-button");
const duplicateRedownloadButton = document.querySelector("#duplicate-redownload-button");
const duplicateCancelButton = document.querySelector("#duplicate-cancel-button");
const themeButtons = Array.from(document.querySelectorAll("[data-theme-mode]"));
const mainPage = document.querySelector("#main-page");
const settingsButton = document.querySelector("#settings-button");
const settingsPage = document.querySelector("#settings-page");
const settingsCloseButton = document.querySelector("#settings-close-button");
const browserModeInput = document.querySelector("#browser-mode-input");
const downloadDirectoryDisplay = document.querySelector("#download-directory-display");
const chooseDirectoryButton = document.querySelector("#choose-directory-button");
const resetDirectoryButton = document.querySelector("#reset-directory-button");
const maxConcurrentInput = document.querySelector("#max-concurrent-input");
const decryptWorkersInput = document.querySelector("#decrypt-workers-input");
const ffmpegPathInput = document.querySelector("#ffmpeg-path-input");
const ffprobePathInput = document.querySelector("#ffprobe-path-input");
const aria2cPathInput = document.querySelector("#aria2c-path-input");
const nodePathInput = document.querySelector("#node-path-input");
const aria2ArgsInput = document.querySelector("#aria2-args-input");
const ffmpegArgsInput = document.querySelector("#ffmpeg-args-input");
const homebrewStatus = document.querySelector("#homebrew-status");
const checkHomebrewButton = document.querySelector("#check-homebrew-button");
const openHomebrewButton = document.querySelector("#open-homebrew-button");
const copyHomebrewCommandButton = document.querySelector("#copy-homebrew-command-button");
const detectToolsButton = document.querySelector("#detect-tools-button");
const installFfmpegButton = document.querySelector("#install-ffmpeg-button");
const installAria2Button = document.querySelector("#install-aria2-button");
const installNodeButton = document.querySelector("#install-node-button");
const settingsStatus = document.querySelector("#settings-status");
const ffmpegToolStatus = document.querySelector("#ffmpeg-tool-status");
const aria2ToolStatus = document.querySelector("#aria2-tool-status");
const nodeToolStatus = document.querySelector("#node-tool-status");
const maintenanceLogOutput = document.querySelector("#maintenance-log-output");
const copyMaintenanceLogButton = document.querySelector("#copy-maintenance-log-button");
const clearMaintenanceLogButton = document.querySelector("#clear-maintenance-log-button");

const appWindow = getCurrentWindow();
const themeQuery = window.matchMedia("(prefers-color-scheme: dark)");
const themePreferenceKey = "streamweave-theme";
const appSettingsKey = "streamweave-settings";
const maxLogLength = 80_000;
const minConcurrentTasks = 1;
const maxConcurrentTasks = 8;
const minDecryptWorkers = 1;
const maxDecryptWorkers = 16;
const browserModes = new Set(["headless", "background", "headed"]);
const runningStatuses = new Set([
  "parsing",
  "fetching_m3u8",
  "downloading",
  "decrypting",
  "merging",
  "verifying"
]);
const terminalStatuses = new Set(["completed", "failed", "cancelled"]);
const statusText = {
  queued: "队列中",
  parsing: "解析中",
  fetching_m3u8: "获取 m3u8",
  downloading: "下载中",
  decrypting: "解密中",
  merging: "合并中",
  verifying: "验证中",
  paused: "已暂停",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消"
};

const taskActionConfig = {
  pause: { command: "pause_download", label: "⏸", title: "暂停任务" },
  resume: { command: "resume_download", label: "▶", title: "继续任务" },
  cancel: { command: "cancel_download", label: "×", title: "取消任务" },
  retry: { command: "retry_download", label: "↻", title: "重试任务" },
  delete: { command: "delete_download_task", label: "⌫", title: "删除记录" }
};

let listenersReady = false;
let selectedTaskId = null;
const selectedTaskIds = new Set();
let isWindowFocused = true;
let notificationPermission = "default";
let themePreference = localStorage.getItem(themePreferenceKey) || "auto";
let notificationBucket = [];
let notificationTimer = null;
const tasks = new Map();
let appSettings = loadSettings();
let isHomebrewAvailable = false;
let isInstallingTool = false;
let maintenanceLogText = "";
let toolStatusRefreshTimer = null;
let duplicateDialogResolver = null;
let scheduledRenderId = 0;
let scheduledTaskRowUpdateId = 0;
let scheduledSelectedDetailUpdateId = 0;
let scheduledSelectedLogFlushId = 0;
const dirtyTaskRowIds = new Set();

startButton.disabled = true;

function isAutoTheme() {
  return themePreference === "auto";
}

function getEffectiveTheme() {
  if (themePreference === "dark") return "dark";
  if (themePreference === "light") return "light";
  return themeQuery.matches ? "dark" : "light";
}

function setThemeAttribute(theme) {
  document.documentElement.dataset.theme = theme;
}

function updateThemeButtons() {
  themeButtons.forEach((button) => {
    const active = button.dataset.themeMode === themePreference;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  });
}

async function applyThemePreference() {
  const effectiveTheme = getEffectiveTheme();
  setThemeAttribute(effectiveTheme);
  updateThemeButtons();
  try {
    await setTheme(isAutoTheme() ? null : themePreference);
  } catch {
    // Theme is best-effort.
  }
}

async function setThemePreference(nextPreference) {
  themePreference = nextPreference;
  localStorage.setItem(themePreferenceKey, nextPreference);
  await applyThemePreference();
}

function defaultSettings() {
  return {
    browserMode: "headed",
    outputDirectory: "",
    maxConcurrentTasks: 3,
    decryptWorkers: 4,
    ffmpegPath: "",
    ffprobePath: "",
    aria2cPath: "",
    nodePath: "",
    aria2Args: "",
    ffmpegArgs: ""
  };
}

function loadSettings() {
  try {
    return normalizeSettings({ ...defaultSettings(), ...JSON.parse(localStorage.getItem(appSettingsKey) || "{}") });
  } catch {
    return defaultSettings();
  }
}

function normalizeSettings(settings) {
  const browserMode = normalizeBrowserMode(settings);
  return {
    ...settings,
    browserMode,
    headlessBrowser: browserMode === "headless",
    maxConcurrentTasks: normalizeConcurrentTasks(settings.maxConcurrentTasks),
    decryptWorkers: normalizeDecryptWorkers(settings.decryptWorkers)
  };
}

function saveSettings() {
  appSettings = readSettingsForm();
  localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
}

function applySettingsForm() {
  browserModeInput.value = normalizeBrowserMode(appSettings);
  updateDownloadDirectoryDisplay();
  maxConcurrentInput.value = String(normalizeConcurrentTasks(appSettings.maxConcurrentTasks));
  decryptWorkersInput.value = String(normalizeDecryptWorkers(appSettings.decryptWorkers));
  ffmpegPathInput.value = appSettings.ffmpegPath || "";
  ffprobePathInput.value = appSettings.ffprobePath || "";
  aria2cPathInput.value = appSettings.aria2cPath || "";
  nodePathInput.value = appSettings.nodePath || "";
  aria2ArgsInput.value = appSettings.aria2Args || "";
  ffmpegArgsInput.value = appSettings.ffmpegArgs || "";
}

function readSettingsForm() {
  const browserMode = normalizeBrowserMode({ browserMode: browserModeInput.value });
  return {
    browserMode,
    headlessBrowser: browserMode === "headless",
    outputDirectory: appSettings.outputDirectory || "",
    maxConcurrentTasks: normalizeConcurrentTasks(maxConcurrentInput.value),
    decryptWorkers: normalizeDecryptWorkers(decryptWorkersInput.value),
    ffmpegPath: ffmpegPathInput.value.trim(),
    ffprobePath: ffprobePathInput.value.trim(),
    aria2cPath: aria2cPathInput.value.trim(),
    nodePath: nodePathInput.value.trim(),
    aria2Args: aria2ArgsInput.value.trim(),
    ffmpegArgs: ffmpegArgsInput.value.trim()
  };
}

function normalizeConcurrentTasks(value) {
  const parsed = Number.parseInt(String(value), 10);
  if (!Number.isFinite(parsed)) return 3;
  return Math.min(Math.max(parsed, minConcurrentTasks), maxConcurrentTasks);
}

function normalizeBrowserMode(settings) {
  if (browserModes.has(settings.browserMode)) return settings.browserMode;
  if (typeof settings.headlessBrowser === "boolean") {
    return settings.headlessBrowser ? "headless" : "background";
  }
  return "headed";
}

function normalizeDecryptWorkers(value) {
  const parsed = Number.parseInt(String(value), 10);
  if (!Number.isFinite(parsed)) return 4;
  return Math.min(Math.max(parsed, minDecryptWorkers), maxDecryptWorkers);
}

async function syncMaxConcurrentTasks({ silent = false } = {}) {
  const count = normalizeConcurrentTasks(appSettings.maxConcurrentTasks);
  maxConcurrentInput.value = String(count);
  appSettings.maxConcurrentTasks = count;
  localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
  try {
    const applied = await invoke("set_max_concurrent_tasks", { count });
    if (applied !== count) {
      appSettings.maxConcurrentTasks = applied;
      maxConcurrentInput.value = String(applied);
      localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
    }
    if (!silent) setSettingsStatus(`同时下载任务数已设置为 ${applied}`);
  } catch (error) {
    if (!silent) setSettingsStatus(String(error), true);
  }
}

async function syncDecryptWorkers({ silent = false } = {}) {
  const count = normalizeDecryptWorkers(appSettings.decryptWorkers);
  decryptWorkersInput.value = String(count);
  appSettings.decryptWorkers = count;
  localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
  try {
    const applied = await invoke("set_decrypt_workers", { count });
    if (applied !== count) {
      appSettings.decryptWorkers = applied;
      decryptWorkersInput.value = String(applied);
      localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
    }
    if (!silent) setSettingsStatus(`全局解密并行数已设置为 ${applied}`);
  } catch (error) {
    if (!silent) setSettingsStatus(String(error), true);
  }
}

function updateDownloadDirectoryDisplay() {
  downloadDirectoryDisplay.textContent = appSettings.outputDirectory || "默认使用 Downloads";
  downloadDirectoryDisplay.title = appSettings.outputDirectory || "默认使用 Downloads";
}

function settingsPayload() {
  saveSettings();
  return {
    browser_mode: appSettings.browserMode,
    headless_browser: appSettings.browserMode === "headless",
    decrypt_workers: appSettings.decryptWorkers,
    ffmpeg_path: appSettings.ffmpegPath || null,
    ffprobe_path: appSettings.ffprobePath || null,
    aria2c_path: appSettings.aria2cPath || null,
    node_path: appSettings.nodePath || null,
    aria2_args: appSettings.aria2Args || null,
    ffmpeg_args: appSettings.ffmpegArgs || null
  };
}

function setSettingsStatus(message, isError = false) {
  settingsStatus.textContent = message;
  settingsStatus.classList.toggle("is-error", isError);
}

function setToolStatus(element, message, state = "") {
  element.textContent = message;
  element.title = message;
  element.classList.toggle("is-ok", state === "ok");
  element.classList.toggle("is-error", state === "error");
  element.classList.toggle("is-checking", state === "checking");
}

function setAllToolStatuses(message, state = "") {
  [ffmpegToolStatus, aria2ToolStatus, nodeToolStatus].forEach((element) => {
    setToolStatus(element, message, state);
  });
}

function applyToolStatusResult(result) {
  const ffmpegOk = Boolean(result.ffmpeg?.ok && result.ffprobe?.ok);
  if (ffmpegOk) {
    setToolStatus(ffmpegToolStatus, result.ffmpeg.path || "可用", "ok");
    ffmpegToolStatus.title = `ffmpeg: ${result.ffmpeg.path || "可用"}\nffprobe: ${result.ffprobe.path || "可用"}`;
  } else {
    const missing = !result.ffmpeg?.ok ? result.ffmpeg : result.ffprobe;
    setToolStatus(ffmpegToolStatus, missing ? missing.message : "未返回", "error");
  }

  const entries = [
    [aria2ToolStatus, result.aria2c],
    [nodeToolStatus, result.node]
  ];
  const otherToolsOk = entries.every(([, item]) => Boolean(item?.ok));
  entries.forEach(([element, item]) => {
    if (!item) {
      setToolStatus(element, "未返回", "error");
      return;
    }
    setToolStatus(element, item.ok ? item.path || "可用" : item.message, item.ok ? "ok" : "error");
  });
  return ffmpegOk && otherToolsOk;
}

function appendMaintenanceLog(message) {
  const timestamp = new Date().toLocaleString("zh-CN", { hour12: false });
  const line = `[${timestamp}] ${String(message).trimEnd()}`;
  maintenanceLogText += `${line}\n`;
  if (maintenanceLogText.length > maxLogLength) {
    maintenanceLogText = `... 已省略较早维护日志 ...\n${maintenanceLogText.slice(-maxLogLength)}`;
  }
  maintenanceLogOutput.textContent = maintenanceLogText || "等待维护操作...";
  maintenanceLogOutput.scrollTop = maintenanceLogOutput.scrollHeight;
  copyMaintenanceLogButton.disabled = maintenanceLogText.length === 0;
  clearMaintenanceLogButton.disabled = maintenanceLogText.length === 0;
}

function clearMaintenanceLog() {
  maintenanceLogText = "";
  maintenanceLogOutput.textContent = "等待维护操作...";
  copyMaintenanceLogButton.disabled = true;
  clearMaintenanceLogButton.disabled = true;
}

function localTaskTimestamp() {
  const date = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function formatToolCheck(result) {
  return ["ffmpeg", "ffprobe", "aria2c", "node"]
    .map((name) => {
      const item = result[name];
      if (!item) return `${name}: 未返回`;
      return `${name}: ${item.ok ? item.path : item.message}`;
    })
    .join(" · ");
}

function setHomebrewAvailability(isAvailable, label) {
  isHomebrewAvailable = isAvailable;
  homebrewStatus.textContent = label;
  homebrewStatus.classList.toggle("is-ok", isAvailable);
  homebrewStatus.classList.toggle("is-error", !isAvailable);
  setInstallButtonsDisabled(!isAvailable || isInstallingTool);
}

function setInstallButtonsDisabled(disabled) {
  installFfmpegButton.disabled = disabled;
  installAria2Button.disabled = disabled;
  installNodeButton.disabled = disabled;
}

function createTask(snapshot) {
  return {
    taskId: snapshot.task_id,
    inputUrl: snapshot.input_url,
    outputDirectory: snapshot.output_directory,
    outputFileName: snapshot.output_file_name,
    status: snapshot.status,
    stage: snapshot.stage,
    progressCompleted: snapshot.progress_completed || 0,
    progressTotal: snapshot.progress_total || 0,
    errorSummary: snapshot.error_summary || "",
    outputPath: snapshot.output_path || "",
    workingDirectory: snapshot.working_directory || "",
    lastLog: snapshot.last_log || "",
    createdAt: snapshot.created_at || "",
    updatedAt: snapshot.updated_at || "",
    completedAt: snapshot.completed_at || "",
    resultSummary: "",
    logs: ""
  };
}

function mergeTaskSnapshot(snapshot) {
  const taskId = snapshot.task_id;
  const existing = tasks.get(taskId);
  const task = existing || createTask(snapshot);
  task.inputUrl = snapshot.input_url ?? task.inputUrl;
  task.outputDirectory = snapshot.output_directory ?? task.outputDirectory;
  task.outputFileName = snapshot.output_file_name ?? task.outputFileName;
  task.status = snapshot.status ?? task.status;
  task.stage = snapshot.stage ?? task.stage;
  task.progressCompleted = snapshot.progress_completed ?? task.progressCompleted;
  task.progressTotal = snapshot.progress_total ?? task.progressTotal;
  task.errorSummary = snapshot.error_summary || "";
  task.outputPath = snapshot.output_path || task.outputPath || "";
  task.workingDirectory = snapshot.working_directory || task.workingDirectory || "";
  task.lastLog = snapshot.last_log || task.lastLog || "";
  task.createdAt = snapshot.created_at || task.createdAt;
  task.updatedAt = snapshot.updated_at || task.updatedAt;
  if (Object.prototype.hasOwnProperty.call(snapshot, "completed_at")) {
    task.completedAt = snapshot.completed_at || "";
  }
  tasks.set(taskId, task);
  if (!selectedTaskId) selectedTaskId = taskId;
  scheduleRender();
}

function mergeTaskHistory(item) {
  mergeTaskSnapshot(item.snapshot);
  const task = tasks.get(item.snapshot.task_id);
  if (!task) return;
  task.logs = item.logs ? `${item.logs.trimEnd()}\n` : "";
  const lines = task.logs.trim().split("\n").filter(Boolean);
  if (lines.length > 0) task.lastLog = lines.at(-1);
  scheduleRender();
}

async function loadPersistedTasks() {
  try {
    const items = await invoke("list_download_tasks");
    items.forEach(mergeTaskHistory);
  } catch (error) {
    summary.textContent = `加载任务历史失败：${error}`;
  }
}

function appendTaskLog(taskId, message) {
  const task = tasks.get(taskId);
  if (!task) return;
  task.logs += message.endsWith("\n") ? message : `${message}\n`;
  if (task.logs.length > maxLogLength) {
    task.logs = `... 已省略较早日志 ...\n${task.logs.slice(-maxLogLength)}`;
  }
  const lines = message.trim().split("\n").filter(Boolean);
  if (lines.length > 0) task.lastLog = lines.at(-1);
  task.updatedAt = localTaskTimestamp();
  markTaskRowDirty(taskId);
  scheduleSelectedLogFlush(taskId);
}

function sortedTasks() {
  return Array.from(tasks.values()).sort((a, b) => {
    const rankDiff = taskSortGroup(a) - taskSortGroup(b);
    if (rankDiff !== 0) return rankDiff;
    const aTime = taskSortTimestamp(a);
    const bTime = taskSortTimestamp(b);
    return bTime - aTime;
  });
}

function taskSortGroup(task) {
  if (task.status === "completed") return 2;
  if (task.status === "failed" || task.status === "cancelled") return 1;
  return 0;
}

function taskSortTimestamp(task) {
  return parseTaskTime(taskSortTime(task).value);
}

function parseTaskTime(value) {
  const normalized = String(value || "")
    .trim()
    .replace(/\//g, "-")
    .replace(" ", "T");
  return Date.parse(normalized) || 0;
}

function taskSortTime(task) {
  if (task.status === "completed") {
    return { label: "完成时间", value: task.completedAt || task.updatedAt || task.createdAt || "" };
  }
  if (task.status === "failed" || task.status === "cancelled") {
    return { label: "活动时间", value: task.updatedAt || task.createdAt || "" };
  }
  return { label: "添加时间", value: task.createdAt || task.updatedAt || "" };
}

function taskName(task) {
  const explicit = task.outputFileName?.trim();
  if (explicit) return explicit.replace(/\.mp4$/i, "");
  const outputName = fileStemFromPath(task.outputPath);
  if (outputName) return outputName;
  try {
    const url = new URL(task.inputUrl);
    const part = url.pathname.split("/").filter(Boolean).at(-1);
    const decoded = part ? decodeURIComponent(part) : "";
    if (decoded && decoded.toLowerCase().endsWith(".m3u8")) {
      return decoded.replace(/\.m3u8$/i, "");
    }
    if (isGenericWebPageName(decoded)) {
      return runningStatuses.has(task.status) || task.status === "queued" ? "正在解析视频信息" : url.hostname;
    }
    return decoded || url.hostname;
  } catch {
    return task.inputUrl || "未命名任务";
  }
}

function isGenericWebPageName(name) {
  if (!name) return true;
  return /^(index|play|player|video|vod|watch|\d+)\.(html?|php|aspx?|jsp)$/i.test(name);
}

function fileStemFromPath(path) {
  if (!path) return "";
  const fileName = String(path).split(/[\\/]/).filter(Boolean).at(-1) || "";
  return fileName.replace(/\.mp4$/i, "").trim();
}

function progressText(task) {
  if (!task.progressTotal) return "0%";
  const percent = Math.round((task.progressCompleted / Math.max(task.progressTotal, 1)) * 100);
  return `${Math.min(percent, 100)}%`;
}

function render() {
  scheduledRenderId = 0;
  const orderedTasks = sortedTasks();
  renderSummary();
  normalizeTaskSelection();
  renderBatchToolbar(orderedTasks);
  renderTaskList(orderedTasks);
  renderDetail();
  void syncDockBadge();
}

function scheduleRender() {
  if (scheduledRenderId) return;
  scheduledRenderId = window.requestAnimationFrame(render);
}

function renderSummary() {
  const all = Array.from(tasks.values());
  const running = all.filter((task) => runningStatuses.has(task.status)).length;
  const queued = all.filter((task) => task.status === "queued").length;
  activeSummary.textContent = `${running} 运行 · ${queued} 排队`;
}

function renderTaskList(items = sortedTasks()) {
  if (items.length === 0) {
    taskList.innerHTML = `<div class="empty-list">暂无任务</div>`;
    return;
  }
  taskList.innerHTML = items
    .map((task) => {
      const selected = task.taskId === selectedTaskId ? " is-selected" : "";
      const checked = selectedTaskIds.has(task.taskId) ? " checked" : "";
      const lastLine = escapeHtml(task.lastLog || task.errorSummary || task.stage || "");
      const name = taskName(task);
      const sortTime = taskSortTime(task);
      return `
        <div class="task-item${selected}" data-task-id="${task.taskId}" title="${escapeHtml(name)}">
          <div class="task-select-cell">
            <input class="task-select-checkbox" type="checkbox" data-task-select="${task.taskId}" aria-label="选择任务"${checked} />
          </div>
          <div class="task-content">
            <div class="task-title-row">
              <strong>${escapeHtml(name)}</strong>
              <span class="status-pill status-${task.status}">${statusText[task.status] || task.status}</span>
              <div class="task-item-actions" aria-label="任务操作">
                ${taskActionButtons(task)}
              </div>
            </div>
            <span class="task-progress-row">
              <span class="mini-progress"><span style="width: ${task.progressTotal ? progressText(task) : "0%"}"></span></span>
              <span class="task-progress-percent">${task.progressTotal ? progressText(task) : "等待"}</span>
            </span>
            <span class="task-meta">${escapeHtml(formatTaskTime(sortTime))}</span>
            <span class="task-last-log">${lastLine || "等待日志"}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

function taskItemElement(taskId) {
  return taskList.querySelector(`[data-task-id="${taskId}"]`);
}

function updateTaskSelection(previousTaskId, nextTaskId) {
  if (previousTaskId) taskItemElement(previousTaskId)?.classList.remove("is-selected");
  if (nextTaskId) taskItemElement(nextTaskId)?.classList.add("is-selected");
}

function selectTask(taskId) {
  if (!taskId || selectedTaskId === taskId) return;
  const previousTaskId = selectedTaskId;
  selectedTaskId = taskId;
  updateTaskSelection(previousTaskId, selectedTaskId);
  renderDetail();
}

function updateTaskRow(task) {
  const item = taskItemElement(task.taskId);
  if (!item) return;
  const progress = item.querySelector(".mini-progress span");
  const percent = item.querySelector(".task-progress-percent");
  const meta = item.querySelector(".task-meta");
  const lastLog = item.querySelector(".task-last-log");
  if (progress) progress.style.width = task.progressTotal ? progressText(task) : "0%";
  if (percent) percent.textContent = task.progressTotal ? progressText(task) : "等待";
  if (meta) meta.textContent = formatTaskTime(taskSortTime(task));
  if (lastLog) lastLog.textContent = task.lastLog || task.errorSummary || task.stage || "等待日志";
}

function formatTaskTime(sortTime) {
  if (!sortTime.value) return sortTime.label;
  return `${sortTime.label}：${sortTime.value}`;
}

function markTaskRowDirty(taskId) {
  dirtyTaskRowIds.add(taskId);
  if (scheduledTaskRowUpdateId) return;
  scheduledTaskRowUpdateId = window.requestAnimationFrame(() => {
    scheduledTaskRowUpdateId = 0;
    const taskIds = Array.from(dirtyTaskRowIds);
    dirtyTaskRowIds.clear();
    taskIds.forEach((id) => {
      const task = tasks.get(id);
      if (task) updateTaskRow(task);
    });
  });
}

function updateSelectedDetailFields(task) {
  if (!task || selectedTaskId !== task.taskId) return;
  detailTitle.textContent = taskName(task);
  detailSubtitle.textContent = task.inputUrl;
  stageLabel.textContent = task.stage || statusText[task.status] || task.status;
  segmentLabel.textContent = `${task.progressCompleted} / ${task.progressTotal} 分片`;
  progress.max = Math.max(task.progressTotal, 1);
  progress.value = Math.min(task.progressCompleted, task.progressTotal);
  summary.textContent = detailSummary(task);
  taskDirectory.textContent = task.workingDirectory ? `临时目录：${task.workingDirectory}` : "";
  setActionState(task);
}

function scheduleSelectedDetailUpdate(taskId) {
  if (selectedTaskId !== taskId || scheduledSelectedDetailUpdateId) return;
  scheduledSelectedDetailUpdateId = window.requestAnimationFrame(() => {
    scheduledSelectedDetailUpdateId = 0;
    updateSelectedDetailFields(tasks.get(selectedTaskId));
  });
}

function scheduleSelectedLogFlush(taskId) {
  if (selectedTaskId !== taskId || scheduledSelectedLogFlushId) return;
  scheduledSelectedLogFlushId = window.requestAnimationFrame(() => {
    scheduledSelectedLogFlushId = 0;
    const task = tasks.get(selectedTaskId);
    if (!task) return;
    logOutput.textContent = task.logs || "等待开始...";
    logOutput.scrollTop = logOutput.scrollHeight;
    summary.textContent = detailSummary(task);
  });
}

function taskActionButtons(task) {
  return availableTaskActions(task)
    .map((action) => {
      const config = taskActionConfig[action];
      return `<button class="task-icon-button" type="button" data-task-action="${action}" data-task-id="${task.taskId}" title="${config.title}" aria-label="${config.title}">${config.label}</button>`;
    })
    .join("");
}

function availableTaskActions(task) {
  const actions = [];
  if (canPause(task)) actions.push("pause");
  if (canResume(task)) actions.push("resume");
  if (canCancel(task)) actions.push("cancel");
  if (canRetry(task)) actions.push("retry");
  if (canDelete(task)) actions.push("delete");
  return actions;
}

function canPause(task) {
  return runningStatuses.has(task?.status);
}

function canResume(task) {
  return task?.status === "paused";
}

function canCancel(task) {
  return task && (task.status === "queued" || task.status === "paused" || runningStatuses.has(task.status));
}

function canRetry(task) {
  return task && (task.status === "failed" || task.status === "cancelled");
}

function canDelete(task) {
  return task && terminalStatuses.has(task.status);
}

function renderDetail() {
  const task = selectedTaskId ? tasks.get(selectedTaskId) : null;
  if (!task) {
    detailTitle.textContent = "未选择任务";
    detailSubtitle.textContent = "提交 URL 后会在这里显示任务详情。";
    stageLabel.textContent = "准备就绪";
    segmentLabel.textContent = "0 / 0 分片";
    progress.max = 1;
    progress.value = 0;
    summary.textContent = "";
    taskDirectory.textContent = "";
    logOutput.textContent = "等待开始...";
    setActionState(null);
    return;
  }

  updateSelectedDetailFields(task);
  logOutput.textContent = task.logs || "等待开始...";
  logOutput.scrollTop = logOutput.scrollHeight;
}

function detailSummary(task) {
  if (task.errorSummary) return task.errorSummary;
  if (task.outputPath && task.resultSummary) return `${task.outputPath} · ${task.resultSummary}`;
  if (task.outputPath) return task.outputPath;
  if (task.outputDirectory) return `输出目录：${task.outputDirectory}`;
  return task.status === "queued" ? "等待空闲下载槽位。" : "";
}

function setActionState(task) {
  const status = task?.status;
  openButton.disabled = !task?.outputPath || status !== "completed";
  revealButton.disabled = !task || !(task.outputPath || task.workingDirectory);
  copyLogButton.disabled = !task;
}

function normalizeTaskSelection() {
  for (const taskId of Array.from(selectedTaskIds)) {
    if (!tasks.has(taskId)) selectedTaskIds.delete(taskId);
  }
}

function selectedTasks() {
  return Array.from(selectedTaskIds)
    .map((taskId) => tasks.get(taskId))
    .filter(Boolean);
}

function renderBatchToolbar(all = sortedTasks()) {
  const selected = selectedTasks();
  selectedSummary.textContent = selected.length ? `已选择 ${selected.length}` : "未选择";
  selectAllTasksInput.checked = all.length > 0 && selected.length === all.length;
  selectAllTasksInput.indeterminate = selected.length > 0 && selected.length < all.length;
  selectAllTasksInput.disabled = all.length === 0;
  batchPauseButton.disabled = !selected.some(canPause);
  batchResumeButton.disabled = !selected.some(canResume);
  batchCancelButton.disabled = !selected.some(canCancel);
  batchRetryButton.disabled = !selected.some(canRetry);
  batchDeleteButton.disabled = !selected.some(canDelete);
}

async function invokeTaskAction(taskId, action) {
  const config = taskActionConfig[action];
  if (!config) return;
  try {
    const payload = action === "retry" ? { taskId, settings: settingsPayload() } : { taskId };
    await invoke(config.command, payload);
  } catch (error) {
    const task = tasks.get(taskId);
    if (task) appendTaskLog(taskId, `${config.title}失败：${error}`);
  }
}

async function runBatchAction(action) {
  const predicate = {
    pause: canPause,
    resume: canResume,
    cancel: canCancel,
    retry: canRetry,
    delete: canDelete
  }[action];
  if (!predicate) return;
  const targets = selectedTasks().filter(predicate);
  for (const task of targets) {
    await invokeTaskAction(task.taskId, action);
  }
}

function normalizedInputUrl(value) {
  try {
    const url = new URL(String(value).trim());
    url.hash = "";
    return url.toString();
  } catch {
    return String(value).trim();
  }
}

function findDuplicateTask(url) {
  const target = normalizedInputUrl(url);
  return Array.from(tasks.values()).find((task) => normalizedInputUrl(task.inputUrl) === target) || null;
}

async function handleDuplicateTask(task) {
  selectTask(task.taskId);
  if (runningStatuses.has(task.status) || task.status === "queued" || task.status === "paused") {
    summary.textContent = "该 URL 已在任务列表中，已为你选中已有任务。";
    return false;
  }
  if (task.status === "failed" || task.status === "cancelled") {
    summary.textContent = "该 URL 已有失败或取消记录，可直接在任务列表中重试。";
    return false;
  }
  if (task.status === "completed") {
    const action = await showDuplicateDialog(task);
    if (action === "open") {
      if (task.outputPath) {
        try {
          await invoke("open_path", { path: task.outputPath });
        } catch (error) {
          const message = `打开文件失败：${error}`;
          summary.textContent = message;
          appendTaskLog(task.taskId, message);
        }
      }
      return false;
    }
    return action === "redownload";
  }
  return true;
}

function showDuplicateDialog(task) {
  duplicateDialogMessage.textContent = `${taskName(task)} 已经下载完成。你可以打开现有文件，或重新创建一个下载任务。`;
  duplicateOpenButton.disabled = !task.outputPath;
  duplicateDialog.hidden = false;
  duplicateRedownloadButton.focus();
  return new Promise((resolve) => {
    duplicateDialogResolver = resolve;
  });
}

function closeDuplicateDialog(action) {
  duplicateDialog.hidden = true;
  const resolve = duplicateDialogResolver;
  duplicateDialogResolver = null;
  if (resolve) resolve(action);
}

async function runDownload() {
  const url = urlInput.value.trim();
  if (!url) {
    summary.textContent = "请输入 m3u8 或视频网页 URL。";
    return;
  }

  await ensureNotificationPermission();
  startButton.disabled = true;
  try {
    const duplicate = findDuplicateTask(url);
    if (duplicate && !(await handleDuplicateTask(duplicate))) {
      return;
    }
    const snapshot = await invoke("start_download", {
      inputUrl: url,
      outputDirectory: appSettings.outputDirectory || null,
      outputFileName: filenameInput.value.trim() || null,
      settings: settingsPayload()
    });
    mergeTaskSnapshot(snapshot);
    selectedTaskId = snapshot.task_id;
    urlInput.value = "";
  } catch (error) {
    summary.textContent = String(error);
  } finally {
    startButton.disabled = !listenersReady;
  }
}

async function openSelectedPath() {
  const task = selectedTaskId ? tasks.get(selectedTaskId) : null;
  if (!task?.outputPath) return;
  try {
    await invoke("open_path", { path: task.outputPath });
  } catch (error) {
    const message = `打开文件失败：${error}`;
    summary.textContent = message;
    appendTaskLog(task.taskId, message);
  }
}

async function revealSelectedPath() {
  const task = selectedTaskId ? tasks.get(selectedTaskId) : null;
  const path = task?.outputPath || task?.workingDirectory;
  if (!path) return;
  try {
    await invoke("reveal_path", { path });
  } catch (error) {
    const message = `在 Finder 中显示失败：${error}`;
    summary.textContent = message;
    appendTaskLog(task.taskId, message);
  }
}

function activeBadgeCount() {
  return Array.from(tasks.values()).filter((task) => runningStatuses.has(task.status) || task.status === "queued").length;
}

async function syncDockBadge() {
  try {
    const count = activeBadgeCount();
    await appWindow.setBadgeCount(count > 0 ? count : undefined);
  } catch {
    // Badge is best-effort.
  }
}

async function refreshWindowFocus() {
  try {
    isWindowFocused = await appWindow.isFocused();
  } catch {
    isWindowFocused = document.hasFocus();
  }
  await syncDockBadge();
}

async function ensureNotificationPermission() {
  if (!("Notification" in window)) {
    notificationPermission = "denied";
    return false;
  }
  if (notificationPermission === "granted") return true;
  if (notificationPermission === "denied") return false;
  try {
    notificationPermission = await Notification.requestPermission();
  } catch {
    notificationPermission = "denied";
  }
  return notificationPermission === "granted";
}

async function showSystemNotification(title, body) {
  if (!(await ensureNotificationPermission())) return;
  try {
    new Notification(title, { body });
  } catch {
    // Ignore notification failures.
  }
}

function enqueueResultNotification(kind, text) {
  notificationBucket.push({ kind, text: compactNotificationText(text) });
  if (notificationTimer !== null) return;
  notificationTimer = window.setTimeout(flushResultNotifications, 900);
}

function flushResultNotifications() {
  const bucket = notificationBucket;
  notificationBucket = [];
  notificationTimer = null;
  if (bucket.length === 0) return;
  if (bucket.length === 1) {
    const item = bucket[0];
    void showSystemNotification(item.kind === "completed" ? "下载完成" : "下载失败", item.text);
    return;
  }
  const completed = bucket.filter((item) => item.kind === "completed").length;
  const failed = bucket.length - completed;
  void showSystemNotification("下载任务更新", `${completed} 个完成，${failed} 个失败`);
}

function compactNotificationText(text, maxLength = 120) {
  const firstLine = String(text).split("\n", 1)[0].trim();
  if (firstLine.length <= maxLength) return firstLine;
  return `${firstLine.slice(0, maxLength - 1)}...`;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

async function registerDownloadEvents() {
  await Promise.all([
    listen("download-task-updated", (event) => {
      mergeTaskSnapshot(event.payload);
    }),
    listen("download-task-deleted", (event) => {
      tasks.delete(event.payload.task_id);
      if (selectedTaskId === event.payload.task_id) {
        selectedTaskId = sortedTasks()[0]?.taskId || null;
      }
      render();
    }),
    listen("download-log", (event) => {
      appendTaskLog(event.payload.task_id, event.payload.message);
    }),
    listen("download-progress", (event) => {
      const task = tasks.get(event.payload.task_id);
      if (!task) return;
      task.progressCompleted = event.payload.completed;
      task.progressTotal = event.payload.total;
      task.updatedAt = localTaskTimestamp();
      markTaskRowDirty(task.taskId);
      scheduleSelectedDetailUpdate(task.taskId);
    }),
    listen("download-stage", (event) => {
      const task = tasks.get(event.payload.task_id);
      if (!task) return;
      task.stage = event.payload.stage;
      task.status = event.payload.status;
      scheduleRender();
    }),
    listen("download-task-directory", (event) => {
      const task = tasks.get(event.payload.task_id);
      if (!task) return;
      task.workingDirectory = event.payload.path;
      scheduleSelectedDetailUpdate(task.taskId);
    }),
    listen("download-completed", (event) => {
      const result = event.payload;
      const task = tasks.get(result.task_id);
    if (task) {
      const finishedAt = localTaskTimestamp();
      task.status = "completed";
      task.stage = "下载完成";
      task.outputPath = result.output_path;
      task.workingDirectory = result.working_directory || "";
      task.errorSummary = "";
      task.lastLog = "下载完成";
      task.updatedAt = finishedAt;
      task.completedAt = finishedAt;
      task.resultSummary = `${result.size_text} · ${result.duration_text} · ${result.video_codec.toUpperCase()} ${result.width}x${result.height} · ${result.audio_codec.toUpperCase()}`;
    }
      enqueueResultNotification("completed", result.output_path);
      render();
    }),
    listen("download-failed", (event) => {
      const payload = event.payload;
      const task = tasks.get(payload.task_id);
      if (task) {
        const cancelled = String(payload.message).includes("取消");
        task.status = cancelled ? "cancelled" : "failed";
        task.stage = cancelled ? "已取消" : "下载失败";
        task.errorSummary = payload.message;
        task.lastLog = payload.message;
        task.updatedAt = localTaskTimestamp();
        task.completedAt = "";
      }
      if (!String(payload.message).includes("取消")) {
        enqueueResultNotification("failed", payload.message);
      }
      render();
    }),
    listen("maintenance-log", (event) => {
      appendMaintenanceLog(event.payload.message);
    })
  ]);

  await loadPersistedTasks();
  listenersReady = true;
  startButton.disabled = false;
  await refreshWindowFocus();
}

taskList.addEventListener("click", (event) => {
  const actionButton = event.target.closest("[data-task-action]");
  if (actionButton) {
    event.stopPropagation();
    void invokeTaskAction(actionButton.dataset.taskId, actionButton.dataset.taskAction);
    return;
  }

  const checkbox = event.target.closest("[data-task-select]");
  if (checkbox) {
    const taskId = checkbox.dataset.taskSelect;
    if (checkbox.checked) selectedTaskIds.add(taskId);
    else selectedTaskIds.delete(taskId);
    render();
    return;
  }

  const item = event.target.closest("[data-task-id]");
  if (!item) return;
  selectTask(item.dataset.taskId);
});

startButton.addEventListener("click", runDownload);
[urlInput, filenameInput].forEach((input) => {
  input.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.isComposing) return;
    event.preventDefault();
    if (!startButton.disabled) void runDownload();
  });
});
selectAllTasksInput.addEventListener("change", () => {
  selectedTaskIds.clear();
  if (selectAllTasksInput.checked) {
    sortedTasks().forEach((task) => selectedTaskIds.add(task.taskId));
  }
  render();
});
batchPauseButton.addEventListener("click", () => void runBatchAction("pause"));
batchResumeButton.addEventListener("click", () => void runBatchAction("resume"));
batchCancelButton.addEventListener("click", () => void runBatchAction("cancel"));
batchRetryButton.addEventListener("click", () => void runBatchAction("retry"));
batchDeleteButton.addEventListener("click", () => void runBatchAction("delete"));
openButton.addEventListener("click", openSelectedPath);
revealButton.addEventListener("click", revealSelectedPath);

copyLogButton.addEventListener("click", async () => {
  const task = selectedTaskId ? tasks.get(selectedTaskId) : null;
  try {
    await navigator.clipboard.writeText(task?.logs || "");
    copyLogButton.textContent = "已复制";
  } catch (error) {
    copyLogButton.textContent = "复制失败";
    if (task) appendTaskLog(task.taskId, `复制日志失败：${error}`);
  }
  window.setTimeout(() => {
    copyLogButton.textContent = "复制日志";
  }, 1200);
});

copyMaintenanceLogButton.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(maintenanceLogText);
    copyMaintenanceLogButton.textContent = "已复制";
  } catch (error) {
    copyMaintenanceLogButton.textContent = "复制失败";
    appendMaintenanceLog(`复制维护日志失败：${error}`);
  }
  window.setTimeout(() => {
    copyMaintenanceLogButton.textContent = "复制日志";
  }, 1200);
});

clearMaintenanceLogButton.addEventListener("click", clearMaintenanceLog);
duplicateOpenButton.addEventListener("click", () => closeDuplicateDialog("open"));
duplicateRedownloadButton.addEventListener("click", () => closeDuplicateDialog("redownload"));
duplicateCancelButton.addEventListener("click", () => closeDuplicateDialog("cancel"));
duplicateDialog.addEventListener("click", (event) => {
  if (event.target === duplicateDialog) closeDuplicateDialog("cancel");
});

themeButtons.forEach((button) => {
  button.addEventListener("click", async () => {
    const nextPreference = button.dataset.themeMode;
    if (!nextPreference) return;
    await setThemePreference(nextPreference);
  });
});

settingsButton.addEventListener("click", () => {
  mainPage.hidden = true;
  settingsPage.hidden = false;
  void refreshMaintenanceStatus({ log: false });
});

settingsCloseButton.addEventListener("click", () => {
  settingsPage.hidden = true;
  mainPage.hidden = false;
});

[
  browserModeInput,
  maxConcurrentInput,
  decryptWorkersInput,
  ffmpegPathInput,
  ffprobePathInput,
  aria2cPathInput,
  nodePathInput,
  aria2ArgsInput,
  ffmpegArgsInput
].forEach((input) => {
  input.addEventListener("input", () => {
    saveSettings();
    setSettingsStatus("设置已保存");
    if (input === maxConcurrentInput) {
      void syncMaxConcurrentTasks();
      return;
    }
    if (input === decryptWorkersInput) {
      void syncDecryptWorkers();
      return;
    }
    if ([ffmpegPathInput, ffprobePathInput, aria2cPathInput, nodePathInput].includes(input)) {
      scheduleToolStatusRefresh();
    }
  });
});

chooseDirectoryButton.addEventListener("click", async () => {
  chooseDirectoryButton.disabled = true;
  setSettingsStatus("正在选择下载目录...");
  try {
    const selected = await invoke("select_directory", {
      currentDirectory: appSettings.outputDirectory || null
    });
    if (selected) {
      appSettings.outputDirectory = selected;
      localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
      updateDownloadDirectoryDisplay();
      setSettingsStatus("下载目录已更新");
    } else {
      setSettingsStatus("已取消选择");
    }
  } catch (error) {
    setSettingsStatus(String(error), true);
  } finally {
    chooseDirectoryButton.disabled = false;
  }
});

resetDirectoryButton.addEventListener("click", () => {
  appSettings.outputDirectory = "";
  localStorage.setItem(appSettingsKey, JSON.stringify(appSettings));
  updateDownloadDirectoryDisplay();
  setSettingsStatus("下载目录已恢复默认");
});

async function checkHomebrewStatus({ log = true } = {}) {
  checkHomebrewButton.disabled = true;
  homebrewStatus.textContent = "检测中...";
  if (log) appendMaintenanceLog("开始检测 Homebrew。");
  try {
    const result = await invoke("check_homebrew");
    setHomebrewAvailability(Boolean(result.ok), result.ok ? result.path || "已安装" : "未安装");
    if (log) {
      appendMaintenanceLog(result.ok ? `Homebrew 已安装：${result.path || result.message}` : `未检测到 Homebrew：${result.message}`);
    }
    if (!result.ok) {
      setSettingsStatus("未检测到 Homebrew，请先安装 Homebrew 或手动配置工具路径。", true);
    } else {
      setSettingsStatus("Homebrew 可用");
    }
  } catch (error) {
    setHomebrewAvailability(false, "检测失败");
    setSettingsStatus(String(error), true);
    if (log) appendMaintenanceLog(`Homebrew 检测失败：${error}`);
  } finally {
    checkHomebrewButton.disabled = false;
  }
}

async function detectToolStatus({ log = true } = {}) {
  detectToolsButton.disabled = true;
  setAllToolStatuses("检测中...", "checking");
  setSettingsStatus("正在检测依赖状态...");
  if (log) appendMaintenanceLog("开始检测 ffmpeg、ffprobe、aria2c、Node。");
  try {
    const result = await invoke("detect_tools", { settings: settingsPayload() });
    const allOk = applyToolStatusResult(result);
    const text = formatToolCheck(result);
    setSettingsStatus(allOk ? "依赖状态正常" : "部分依赖不可用", !allOk);
    if (log) appendMaintenanceLog(text);
    return result;
  } catch (error) {
    setAllToolStatuses("检测失败", "error");
    setSettingsStatus(String(error), true);
    if (log) appendMaintenanceLog(`工具检测失败：${error}`);
    return null;
  } finally {
    detectToolsButton.disabled = false;
  }
}

async function refreshMaintenanceStatus({ log = false } = {}) {
  await checkHomebrewStatus({ log });
  await detectToolStatus({ log });
}

function scheduleToolStatusRefresh() {
  if (toolStatusRefreshTimer !== null) {
    window.clearTimeout(toolStatusRefreshTimer);
  }
  toolStatusRefreshTimer = window.setTimeout(() => {
    toolStatusRefreshTimer = null;
    void detectToolStatus({ log: false });
  }, 500);
}

async function loadAppVersion() {
  try {
    const version = await getVersion();
    const label = version ? `v${version}` : "";
    appVersionLabel.textContent = label;
    settingsAppVersionLabel.textContent = label ? `· ${label}` : "";
    appVersionLabel.title = label ? `StreamWeave ${label}` : "";
    settingsAppVersionLabel.title = label ? `StreamWeave ${label}` : "";
  } catch {
    appVersionLabel.textContent = "";
    settingsAppVersionLabel.textContent = "";
  }
}

checkHomebrewButton.addEventListener("click", () => {
  void checkHomebrewStatus({ log: true });
});

openHomebrewButton.addEventListener("click", async () => {
  try {
    await invoke("open_homebrew_site");
    setSettingsStatus("已打开 Homebrew 官网");
    appendMaintenanceLog("已打开 Homebrew 官网。");
  } catch (error) {
    setSettingsStatus(String(error), true);
    appendMaintenanceLog(`打开 Homebrew 官网失败：${error}`);
  }
});

copyHomebrewCommandButton.addEventListener("click", async () => {
  const command = '/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"';
  try {
    await navigator.clipboard.writeText(command);
    setSettingsStatus("已复制 Homebrew 官方安装命令");
    appendMaintenanceLog("已复制 Homebrew 官方安装命令。");
  } catch (error) {
    setSettingsStatus(`复制失败：${error}`, true);
    appendMaintenanceLog(`复制 Homebrew 安装命令失败：${error}`);
  }
});

detectToolsButton.addEventListener("click", async () => {
  await detectToolStatus({ log: true });
});

async function installWithHomebrew(button, command, name) {
  isInstallingTool = true;
  setInstallButtonsDisabled(true);
  setSettingsStatus(`正在执行 brew install ${name}...`);
  appendMaintenanceLog(`开始执行 brew install ${name}。`);
  try {
    const result = await invoke(command);
    setSettingsStatus(compactNotificationText(result, 180));
    appendMaintenanceLog(result);
    await refreshMaintenanceStatus({ log: false });
  } catch (error) {
    setSettingsStatus(String(error), true);
    appendMaintenanceLog(`brew install ${name} 失败：${error}`);
  } finally {
    isInstallingTool = false;
    setInstallButtonsDisabled(!isHomebrewAvailable);
  }
}

installFfmpegButton.addEventListener("click", () => {
  void installWithHomebrew(installFfmpegButton, "install_ffmpeg", "ffmpeg");
});

installAria2Button.addEventListener("click", () => {
  void installWithHomebrew(installAria2Button, "install_aria2", "aria2");
});

installNodeButton.addEventListener("click", () => {
  void installWithHomebrew(installNodeButton, "install_node", "node");
});

themeQuery.addEventListener("change", () => {
  if (!isAutoTheme()) return;
  void applyThemePreference();
});

window.addEventListener("focus", () => {
  isWindowFocused = true;
  void syncDockBadge();
});

window.addEventListener("blur", () => {
  isWindowFocused = false;
  void syncDockBadge();
});

document.addEventListener("visibilitychange", () => {
  isWindowFocused = document.visibilityState === "visible" && document.hasFocus();
  void syncDockBadge();
});

applyThemePreference();
void loadAppVersion();
applySettingsForm();
void syncMaxConcurrentTasks({ silent: true });
void syncDecryptWorkers({ silent: true });
registerDownloadEvents().catch((error) => {
  stageLabel.textContent = "初始化失败";
  summary.textContent = `日志监听初始化失败：${error}`;
});
render();
