use aes::Aes128;
use cbc::Decryptor;
use chrono::Local;
use cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State};
use url::Url;

const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126 Safari/537.36";

struct AppState {
    next_task_id: AtomicU64,
    tasks: Mutex<TaskRegistry>,
    max_concurrent_tasks: AtomicU64,
    max_decrypt_workers: AtomicU64,
    decrypt_limiter: Arc<DecryptLimiter>,
}

impl Default for AppState {
    fn default() -> Self {
        let tasks = load_task_registry().unwrap_or_default();
        let next_task_id = next_task_sequence(&tasks);
        Self {
            next_task_id: AtomicU64::new(next_task_id),
            tasks: Mutex::new(tasks),
            max_concurrent_tasks: AtomicU64::new(DEFAULT_CONCURRENT_TASKS as u64),
            max_decrypt_workers: AtomicU64::new(DEFAULT_DECRYPT_WORKERS as u64),
            decrypt_limiter: Arc::new(DecryptLimiter::default()),
        }
    }
}

const DEFAULT_CONCURRENT_TASKS: usize = 3;
const MIN_CONCURRENT_TASKS: usize = 1;
const MAX_CONCURRENT_TASKS: usize = 8;
const DEFAULT_DECRYPT_WORKERS: usize = 4;
const MIN_DECRYPT_WORKERS: usize = 1;
const MAX_DECRYPT_WORKERS: usize = 16;
const MAX_TASK_LOG_LINES: usize = 2_000;
const TASK_HISTORY_PERSIST_INTERVAL: Duration = Duration::from_millis(1200);

struct TaskRegistry {
    tasks: HashMap<String, TaskRecord>,
    queue: VecDeque<String>,
    running: HashSet<String>,
    last_persisted_at: Option<Instant>,
}

#[derive(Default)]
struct DecryptLimiter {
    active: Mutex<usize>,
    changed: Condvar,
}

struct DecryptPermit {
    limiter: Arc<DecryptLimiter>,
}

impl Drop for DecryptPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.limiter.active.lock() {
            *active = active.saturating_sub(1);
            self.limiter.changed.notify_one();
        }
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
            queue: VecDeque::new(),
            running: HashSet::new(),
            last_persisted_at: None,
        }
    }
}

struct TaskRecord {
    id: String,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
    status: TaskStatus,
    previous_status: TaskStatus,
    stage: String,
    progress_completed: usize,
    progress_total: usize,
    error_summary: Option<String>,
    output_path: Option<String>,
    working_directory: Option<String>,
    logs: VecDeque<String>,
    control: Arc<TaskControl>,
    worker_started: bool,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
    settings: AppSettings,
}

impl TaskRecord {
    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            task_id: self.id.clone(),
            input_url: self.input_url.clone(),
            output_directory: self.output_directory.clone(),
            output_file_name: self.output_file_name.clone(),
            status: self.status.clone(),
            stage: self.stage.clone(),
            progress_completed: self.progress_completed,
            progress_total: self.progress_total,
            error_summary: self.error_summary.clone(),
            output_path: self.output_path.clone(),
            working_directory: self.working_directory.clone(),
            last_log: self.logs.back().cloned(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            completed_at: self.completed_at.clone(),
        }
    }
}

#[derive(Default)]
struct TaskControl {
    cancel_requested: AtomicBool,
    pause_requested: AtomicBool,
    decrypt_wait_logged: AtomicBool,
    active_pid: Mutex<Option<u32>>,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    Queued,
    Parsing,
    FetchingM3u8,
    Downloading,
    Decrypting,
    Merging,
    Verifying,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self::Queued
    }
}

#[derive(Serialize, Clone)]
struct TaskSnapshot {
    task_id: String,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
    status: TaskStatus,
    stage: String,
    progress_completed: usize,
    progress_total: usize,
    error_summary: Option<String>,
    output_path: Option<String>,
    working_directory: Option<String>,
    last_log: Option<String>,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
}

#[derive(Serialize, Clone)]
struct TaskHistorySnapshot {
    snapshot: TaskSnapshot,
    logs: String,
}

#[derive(Deserialize, Serialize)]
struct PersistedTaskRecord {
    id: String,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
    status: TaskStatus,
    previous_status: TaskStatus,
    stage: String,
    progress_completed: usize,
    progress_total: usize,
    error_summary: Option<String>,
    output_path: Option<String>,
    working_directory: Option<String>,
    logs: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    completed_at: Option<String>,
    settings: AppSettings,
}

#[derive(Deserialize, Serialize)]
struct PersistedTaskIndex {
    version: u8,
    task_ids: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct PersistedTaskMetadata {
    id: String,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
    status: TaskStatus,
    previous_status: TaskStatus,
    stage: String,
    progress_completed: usize,
    progress_total: usize,
    error_summary: Option<String>,
    output_path: Option<String>,
    working_directory: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    completed_at: Option<String>,
    settings: AppSettings,
}

#[derive(Clone)]
struct TaskContext {
    app: tauri::AppHandle,
    task_id: String,
    control: Arc<TaskControl>,
    settings: AppSettings,
}

#[derive(Deserialize, Serialize, Clone)]
struct AppSettings {
    #[serde(default = "default_browser_mode")]
    browser_mode: Option<String>,
    #[serde(default = "default_headless_browser")]
    headless_browser: bool,
    #[serde(default = "default_decrypt_workers")]
    decrypt_workers: usize,
    #[serde(default)]
    ffmpeg_path: Option<String>,
    #[serde(default)]
    ffprobe_path: Option<String>,
    #[serde(default)]
    aria2c_path: Option<String>,
    #[serde(default)]
    node_path: Option<String>,
    #[serde(default)]
    aria2_args: Option<String>,
    #[serde(default)]
    ffmpeg_args: Option<String>,
}

fn default_headless_browser() -> bool {
    false
}

fn default_browser_mode() -> Option<String> {
    Some("headed".to_string())
}

fn default_decrypt_workers() -> usize {
    DEFAULT_DECRYPT_WORKERS
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            browser_mode: Some("headed".to_string()),
            headless_browser: false,
            decrypt_workers: DEFAULT_DECRYPT_WORKERS,
            ffmpeg_path: None,
            ffprobe_path: None,
            aria2c_path: None,
            node_path: None,
            aria2_args: None,
            ffmpeg_args: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserLaunchMode {
    Headless,
    Background,
    Headed,
}

impl BrowserLaunchMode {
    fn from_settings(settings: &AppSettings) -> Self {
        match settings.browser_mode.as_deref() {
            Some("headless") => Self::Headless,
            Some("background") => Self::Background,
            Some("headed") => Self::Headed,
            _ if settings.headless_browser => Self::Headless,
            _ => Self::Background,
        }
    }

    fn script_arg(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Background => "background",
            Self::Headed => "headed",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Headless => "无头模式",
            Self::Background => "后台窗口",
            Self::Headed => "可见窗口",
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Headless => "Playwright 无头浏览器已启动",
            Self::Background => "Playwright 后台浏览器已启动",
            Self::Headed => "Playwright 浏览器已打开",
        }
    }
}

#[derive(Serialize, Clone)]
struct ToolCheckPayload {
    ffmpeg: ToolCheckItem,
    ffprobe: ToolCheckItem,
    aria2c: ToolCheckItem,
    node: ToolCheckItem,
}

#[derive(Serialize, Clone)]
struct ToolCheckItem {
    name: String,
    path: Option<String>,
    ok: bool,
    message: String,
}

type HomebrewCheckPayload = ToolCheckItem;

#[derive(Serialize, Clone)]
struct ProgressPayload {
    task_id: String,
    completed: usize,
    total: usize,
}

#[derive(Serialize, Clone)]
struct DownloadResult {
    task_id: String,
    output_path: String,
    working_directory: String,
    duration_text: String,
    size_text: String,
    video_codec: String,
    audio_codec: String,
    width: u32,
    height: u32,
}

#[derive(Serialize, Clone)]
struct ErrorPayload {
    task_id: String,
    message: String,
}

#[derive(Serialize, Clone)]
struct LogPayload {
    task_id: String,
    message: String,
}

#[derive(Serialize, Clone)]
struct MaintenanceLogPayload {
    message: String,
}

#[derive(Serialize, Clone)]
struct StagePayload {
    task_id: String,
    stage: String,
    status: TaskStatus,
}

#[derive(Serialize, Clone)]
struct TaskDirectoryPayload {
    task_id: String,
    path: String,
}

#[derive(Serialize, Clone)]
struct TaskDeletedPayload {
    task_id: String,
}

#[derive(Default)]
struct ProbeResult {
    duration: f64,
    size: u64,
    video_codec: String,
    audio_codec: String,
    width: u32,
    height: u32,
}

#[tauri::command]
fn cancel_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let snapshot = {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        registry.queue.retain(|id| id != &task_id);
        let Some(task) = registry.tasks.get_mut(&task_id) else {
            return Err("任务不存在。".to_string());
        };
        task.control.cancel_requested.store(true, Ordering::SeqCst);
        task.control.pause_requested.store(false, Ordering::SeqCst);
        if matches!(task.status, TaskStatus::Queued) && !task.worker_started {
            task.status = TaskStatus::Cancelled;
            task.stage = "已取消".to_string();
            task.error_summary = Some("下载已取消。".to_string());
            task.updated_at = timestamp();
        }
        signal_active_process(&task.control, "TERM");
        let snapshot = task.snapshot();
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&app, snapshot)?;
    schedule_tasks(app)
}

#[tauri::command]
fn pause_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let snapshot = {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&task_id) else {
            return Err("任务不存在。".to_string());
        };
        if !is_active_status(&task.status) {
            return Err("只有运行中的任务可以暂停。".to_string());
        }
        task.control.pause_requested.store(true, Ordering::SeqCst);
        task.previous_status = task.status.clone();
        task.status = TaskStatus::Paused;
        task.stage = "已暂停".to_string();
        task.updated_at = timestamp();
        signal_active_process(&task.control, "STOP");
        let snapshot = task.snapshot();
        registry.running.remove(&task_id);
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&app, snapshot)?;
    schedule_tasks(app)
}

#[tauri::command]
fn resume_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let snapshot = {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&task_id) else {
            return Err("任务不存在。".to_string());
        };
        if task.status != TaskStatus::Paused {
            return Err("只有暂停中的任务可以继续。".to_string());
        }
        task.status = TaskStatus::Queued;
        task.stage = "等待继续槽位".to_string();
        task.updated_at = timestamp();
        let snapshot = task.snapshot();
        if !registry.queue.iter().any(|id| id == &task_id) {
            registry.queue.push_back(task_id);
        }
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&app, snapshot)?;
    schedule_tasks(app)
}

#[tauri::command]
fn retry_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    settings: Option<AppSettings>,
) -> Result<(), String> {
    let snapshot = {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        registry.queue.retain(|id| id != &task_id);
        let Some(task) = registry.tasks.get_mut(&task_id) else {
            return Err("任务不存在。".to_string());
        };
        if !matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled) {
            return Err("只有失败或已取消的任务可以重试。".to_string());
        }
        task.status = TaskStatus::Queued;
        task.previous_status = TaskStatus::Queued;
        task.stage = "队列中".to_string();
        task.progress_completed = 0;
        task.progress_total = 0;
        task.error_summary = None;
        task.output_path = None;
        task.working_directory = None;
        task.completed_at = None;
        task.logs.clear();
        task.control = Arc::new(TaskControl::default());
        task.worker_started = false;
        if let Some(settings) = settings {
            let decrypt_workers = settings
                .decrypt_workers
                .clamp(MIN_DECRYPT_WORKERS, MAX_DECRYPT_WORKERS);
            state
                .max_decrypt_workers
                .store(decrypt_workers as u64, Ordering::SeqCst);
            state.decrypt_limiter.changed.notify_all();
            task.settings = settings;
        }
        task.updated_at = timestamp();
        let snapshot = task.snapshot();
        registry.queue.push_back(task_id);
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&app, snapshot)?;
    schedule_tasks(app)
}

#[tauri::command]
fn delete_download_task(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get(&task_id) else {
            return Err("任务不存在。".to_string());
        };
        if !is_terminal_status(&task.status) {
            return Err("只能删除已完成、失败或已取消的任务记录。".to_string());
        }
        cleanup_task_temporary_files(task)?;
        registry.queue.retain(|id| id != &task_id);
        registry.running.remove(&task_id);
        registry.tasks.remove(&task_id);
        persist_task_registry(&mut registry)?;
        cleanup_persisted_task_files(&task_id);
    }
    app.emit("download-task-deleted", TaskDeletedPayload { task_id })
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let target = PathBuf::from(path);
    if !target.exists() {
        return Err("路径不存在，无法在 Finder 中显示。".to_string());
    }
    let status = Command::new("open")
        .arg("-R")
        .arg(target)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("Finder 显示失败，退出码 {:?}。", status.code()));
    }
    Ok(())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let target = PathBuf::from(path);
    if !target.exists() {
        return Err("文件不存在，无法打开。".to_string());
    }
    let status = Command::new("open")
        .arg(target)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("打开文件失败，退出码 {:?}。", status.code()));
    }
    Ok(())
}

#[tauri::command]
fn select_directory(current_directory: Option<String>) -> Result<Option<String>, String> {
    let prompt = "选择下载目录";
    let script = current_directory
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.exists() && path.is_dir())
        .map(|path| {
            format!(
                "POSIX path of (choose folder with prompt \"{}\" default location POSIX file \"{}\")",
                escape_applescript_string(prompt),
                escape_applescript_string(&path.to_string_lossy())
            )
        })
        .unwrap_or_else(|| {
            format!(
                "POSIX path of (choose folder with prompt \"{}\")",
                escape_applescript_string(prompt)
            )
        });

    let output = Command::new("osascript")
        .env("AppleLanguages", "(zh-Hans,zh-CN,en)")
        .env("AppleLocale", "zh_CN")
        .env("LANG", "zh_CN.UTF-8")
        .env("LC_ALL", "zh_CN.UTF-8")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| format!("打开目录选择器失败：{error}"))?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!path.is_empty()).then_some(path));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Ok(None);
    }
    Err(format!("选择目录失败。{}", command_tail(&stderr)))
}

#[tauri::command]
fn set_max_concurrent_tasks(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    count: usize,
) -> Result<usize, String> {
    let next = count.clamp(MIN_CONCURRENT_TASKS, MAX_CONCURRENT_TASKS);
    state
        .max_concurrent_tasks
        .store(next as u64, Ordering::SeqCst);
    schedule_tasks(app)?;
    Ok(next)
}

#[tauri::command]
fn set_decrypt_workers(state: State<'_, AppState>, count: usize) -> usize {
    let next = count.clamp(MIN_DECRYPT_WORKERS, MAX_DECRYPT_WORKERS);
    state
        .max_decrypt_workers
        .store(next as u64, Ordering::SeqCst);
    state.decrypt_limiter.changed.notify_all();
    next
}

#[tauri::command]
fn list_download_tasks(state: State<'_, AppState>) -> Result<Vec<TaskHistorySnapshot>, String> {
    let registry = state.tasks.lock().map_err(|error| error.to_string())?;
    let mut items = registry
        .tasks
        .values()
        .map(|task| TaskHistorySnapshot {
            snapshot: task.snapshot(),
            logs: task.logs.iter().cloned().collect::<Vec<_>>().join("\n"),
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        task_snapshot_sort_group(&a.snapshot)
            .cmp(&task_snapshot_sort_group(&b.snapshot))
            .then_with(|| {
                task_snapshot_sort_time(&b.snapshot).cmp(task_snapshot_sort_time(&a.snapshot))
            })
    });
    Ok(items)
}

fn task_snapshot_sort_group(snapshot: &TaskSnapshot) -> u8 {
    match &snapshot.status {
        TaskStatus::Completed => 2,
        TaskStatus::Failed | TaskStatus::Cancelled => 1,
        _ => 0,
    }
}

fn task_snapshot_sort_time(snapshot: &TaskSnapshot) -> &str {
    match &snapshot.status {
        TaskStatus::Completed => snapshot
            .completed_at
            .as_deref()
            .unwrap_or(&snapshot.updated_at),
        TaskStatus::Failed | TaskStatus::Cancelled => &snapshot.updated_at,
        _ => &snapshot.created_at,
    }
}

#[tauri::command]
fn detect_tools(settings: Option<AppSettings>) -> ToolCheckPayload {
    let settings = settings.unwrap_or_default();
    ToolCheckPayload {
        ffmpeg: tool_check_item("ffmpeg", settings.ffmpeg_path.as_deref()),
        ffprobe: tool_check_item("ffprobe", settings.ffprobe_path.as_deref()),
        aria2c: tool_check_item("aria2c", settings.aria2c_path.as_deref()),
        node: tool_check_item("node", settings.node_path.as_deref()),
    }
}

#[tauri::command]
fn check_homebrew() -> HomebrewCheckPayload {
    tool_check_item("brew", None)
}

#[tauri::command]
fn open_homebrew_site() -> Result<(), String> {
    Command::new("open")
        .arg("https://brew.sh/")
        .status()
        .map_err(|error| format!("打开 Homebrew 官网失败：{error}"))?;
    Ok(())
}

fn emit_maintenance_log(app: &tauri::AppHandle, message: impl Into<String>) {
    let _ = app.emit(
        "maintenance-log",
        MaintenanceLogPayload {
            message: message.into(),
        },
    );
}

fn brew_install_formula(
    app: tauri::AppHandle,
    formula: &str,
    success_message: &str,
) -> Result<String, String> {
    let brew = find_tool("brew")?;
    emit_maintenance_log(&app, format!("$ {} install {}", brew.display(), formula));

    let mut child = Command::new(&brew)
        .args(["install", formula])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动安装命令失败：{error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取安装命令输出。".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取安装命令错误输出。".to_string())?;
    let (tx, rx) = mpsc::channel::<String>();
    spawn_pipe_reader(stdout, tx.clone());
    spawn_pipe_reader(stderr, tx.clone());
    drop(tx);

    let mut recent_output = VecDeque::with_capacity(12);
    loop {
        while let Ok(line) = rx.try_recv() {
            emit_maintenance_log(&app, line.trim_end().to_string());
            remember_line(&mut recent_output, &line);
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("等待安装命令失败：{error}"))?
        {
            while let Ok(line) = rx.try_recv() {
                emit_maintenance_log(&app, line.trim_end().to_string());
                remember_line(&mut recent_output, &line);
            }

            if status.success() {
                emit_maintenance_log(&app, success_message);
                return Ok(if recent_output.is_empty() {
                    success_message.to_string()
                } else {
                    format!(
                        "{} {}",
                        success_message,
                        recent_lines_summary(&recent_output)
                    )
                });
            }

            let message = format!(
                "安装失败，退出码 {:?}。{}",
                status.code(),
                recent_lines_summary(&recent_output)
            );
            emit_maintenance_log(&app, message.clone());
            return Err(message);
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tauri::command]
fn install_ffmpeg(app: tauri::AppHandle) -> Result<String, String> {
    brew_install_formula(app, "ffmpeg", "ffmpeg 安装完成。")
}

#[tauri::command]
fn install_aria2(app: tauri::AppHandle) -> Result<String, String> {
    brew_install_formula(app, "aria2", "aria2 安装完成。")
}

#[tauri::command]
fn install_node(app: tauri::AppHandle) -> Result<String, String> {
    brew_install_formula(app, "node", "Node 安装完成。")
}

#[tauri::command]
fn start_download(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
    settings: Option<AppSettings>,
) -> Result<TaskSnapshot, String> {
    let input = input_url.trim();
    if input.is_empty() {
        return Err("请输入 m3u8 或视频网页 URL。".to_string());
    }
    Url::parse(input).map_err(|_| "请输入有效 URL。".to_string())?;

    let task_id = format!("task-{}", state.next_task_id.fetch_add(1, Ordering::SeqCst));
    let now = timestamp();
    let settings = settings.unwrap_or_default();
    let decrypt_workers = settings
        .decrypt_workers
        .clamp(MIN_DECRYPT_WORKERS, MAX_DECRYPT_WORKERS);
    state
        .max_decrypt_workers
        .store(decrypt_workers as u64, Ordering::SeqCst);
    state.decrypt_limiter.changed.notify_all();
    let record = TaskRecord {
        id: task_id.clone(),
        input_url: input.to_string(),
        output_directory,
        output_file_name,
        status: TaskStatus::Queued,
        previous_status: TaskStatus::Queued,
        stage: "队列中".to_string(),
        progress_completed: 0,
        progress_total: 0,
        error_summary: None,
        output_path: None,
        working_directory: None,
        logs: VecDeque::new(),
        control: Arc::new(TaskControl::default()),
        worker_started: false,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
        settings,
    };
    let snapshot = record.snapshot();
    {
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        registry.queue.push_back(task_id);
        registry.tasks.insert(snapshot.task_id.clone(), record);
        persist_task_registry(&mut registry)?;
    }
    emit_task_update(&app, snapshot.clone())?;
    schedule_tasks(app)?;
    Ok(snapshot)
}

fn start_download_blocking(
    ctx: TaskContext,
    input_url: String,
    output_directory: Option<String>,
    output_file_name: Option<String>,
) -> Result<DownloadResult, String> {
    let input = Url::parse(&input_url).map_err(|_| "请输入有效 URL。".to_string())?;
    let extracted = if input.as_str().to_lowercase().contains(".m3u8") {
        emit_log(&ctx, "输入已经是 m3u8，跳过网页提取。\n")?;
        let headers = inferred_direct_m3u8_headers(&input);
        log_request_headers(&ctx, &headers)?;
        ExtractedM3u8 {
            url: input,
            headers,
            playlist_text: None,
            suggested_name: None,
        }
    } else {
        extract_m3u8_url(&ctx, &input)?
    };

    if let Some(name) = &extracted.suggested_name {
        update_task_suggested_name(&ctx, name)?;
    }
    emit_log(&ctx, &format!("最终使用 m3u8: {}\n", extracted.url))?;
    execute_download(
        ctx,
        extracted.url,
        extracted.headers,
        extracted.playlist_text,
        extracted.suggested_name,
        output_directory,
        output_file_name,
    )
}

fn execute_download(
    ctx: TaskContext,
    input: Url,
    headers: RequestHeaders,
    playlist_text: Option<String>,
    suggested_name: Option<String>,
    output_directory: Option<String>,
    output_file_name: Option<String>,
) -> Result<DownloadResult, String> {
    let output_dir = output_directory
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_downloads_dir);
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;

    let task_parent_dir = task_workspace_parent_dir();
    fs::create_dir_all(&task_parent_dir).map_err(|error| error.to_string())?;
    let working_dir = task_parent_dir.join(format!(
        "{}-{}-{}",
        Local::now().format("%Y%m%d-%H%M%S"),
        ctx.task_id,
        stable_hash(input.as_str())
    ));
    fs::create_dir_all(working_dir.join("parts")).map_err(|error| error.to_string())?;
    emit_task_directory(&ctx, &working_dir)?;

    let output_path = output_dir.join(output_name(
        &input,
        output_file_name,
        suggested_name.as_deref(),
    ));

    update_task_paths(&ctx, Some(&working_dir), Some(&output_path))?;
    emit_stage(&ctx, TaskStatus::FetchingM3u8, "正在获取 m3u8")?;
    emit_log(&ctx, &format!("任务目录: {}\n", working_dir.display()))?;
    emit_log(&ctx, &format!("输出文件: {}\n", output_path.display()))?;
    if output_path.exists() {
        emit_log(&ctx, "目标输出文件已存在，本次任务会覆盖该文件。\n")?;
    }

    let manifest = resolve_m3u8(&input, &working_dir, &headers, playlist_text)?;
    emit_log(
        &ctx,
        &format!("解析到 {} 个分片。\n", manifest.segment_entries.len()),
    )?;
    emit_progress(&ctx, 0, manifest.segment_entries.len())?;
    check_control(&ctx)?;

    emit_stage(&ctx, TaskStatus::FetchingM3u8, "检查依赖工具")?;
    let tools = check_tools(&ctx.settings)?;
    emit_log(&ctx, &format!("ffmpeg: {}\n", tools.ffmpeg.display()))?;
    emit_log(&ctx, &format!("ffprobe: {}\n", tools.ffprobe.display()))?;
    emit_log(&ctx, &format!("aria2c: {}\n", tools.aria2c.display()))?;

    emit_stage(&ctx, TaskStatus::Downloading, "正在下载分片")?;
    download_segments(
        &ctx,
        &tools.aria2c,
        &working_dir,
        manifest.segment_entries.len(),
        &headers,
    )?;
    check_control(&ctx)?;

    emit_stage(&ctx, TaskStatus::Decrypting, "正在解密分片")?;
    decrypt_segments(&ctx, &working_dir, &manifest.segment_entries)?;
    check_control(&ctx)?;

    emit_stage(&ctx, TaskStatus::Merging, "正在检查分片媒体流")?;
    validate_segments_before_merge(
        &ctx,
        &tools.ffprobe,
        &working_dir,
        manifest.segment_entries.len(),
    )?;
    check_control(&ctx)?;

    emit_stage(&ctx, TaskStatus::Merging, "正在合并 mp4")?;
    merge_media(&ctx, &tools.ffmpeg, &working_dir, &output_path)?;
    validate_output_file(&output_path)?;
    check_control(&ctx)?;

    emit_stage(&ctx, TaskStatus::Verifying, "正在验证输出文件")?;
    let probe = verify_media(&ctx, &tools.ffprobe, &output_path)?;

    emit_stage(&ctx, TaskStatus::Verifying, "正在清理临时文件")?;
    cleanup_working_directory(&ctx, &working_dir);
    emit_stage(&ctx, TaskStatus::Completed, "下载完成")?;

    Ok(DownloadResult {
        task_id: ctx.task_id,
        output_path: output_path.to_string_lossy().to_string(),
        working_directory: String::new(),
        duration_text: format_duration(probe.duration),
        size_text: format_size(probe.size),
        video_codec: probe.video_codec,
        audio_codec: probe.audio_codec,
        width: probe.width,
        height: probe.height,
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            start_download,
            cancel_download,
            pause_download,
            resume_download,
            retry_download,
            delete_download_task,
            list_download_tasks,
            detect_tools,
            check_homebrew,
            open_homebrew_site,
            install_ffmpeg,
            install_aria2,
            install_node,
            set_max_concurrent_tasks,
            set_decrypt_workers,
            select_directory,
            reveal_path,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    aria2c: PathBuf,
}

struct Manifest {
    segment_entries: Vec<SegmentEntry>,
}

#[derive(Clone)]
struct SegmentEntry {
    url: Url,
    sequence: u64,
    encryption: Option<SegmentEncryption>,
}

#[derive(Clone)]
struct SegmentEncryption {
    key_bytes: Vec<u8>,
    iv: Option<[u8; 16]>,
}

enum PlaylistKind {
    Media,
    Master,
}

#[derive(Clone, Default)]
struct RequestHeaders {
    referer: Option<String>,
    origin: Option<String>,
    cookie: Option<String>,
    user_agent: Option<String>,
    accept: Option<String>,
    accept_language: Option<String>,
}

struct ExtractedM3u8 {
    url: Url,
    headers: RequestHeaders,
    playlist_text: Option<String>,
    suggested_name: Option<String>,
}

struct BrowserCandidate {
    url: Url,
    headers: RequestHeaders,
    playlist_text: Option<String>,
    suggested_name: Option<String>,
}

fn schedule_tasks(app: tauri::AppHandle) -> Result<(), String> {
    let mut starts = Vec::new();
    {
        let state = app.state::<AppState>();
        let max_concurrent = state
            .max_concurrent_tasks
            .load(Ordering::SeqCst)
            .clamp(MIN_CONCURRENT_TASKS as u64, MAX_CONCURRENT_TASKS as u64)
            as usize;
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        while registry.running.len() < max_concurrent {
            let Some(task_id) = registry.queue.pop_front() else {
                break;
            };
            if registry.running.contains(&task_id) {
                continue;
            }
            let Some(task) = registry.tasks.get_mut(&task_id) else {
                continue;
            };
            if is_terminal_status(&task.status) {
                continue;
            }

            let start = if task.worker_started {
                task.status = task.previous_status.clone();
                task.stage = status_stage(&task.status).to_string();
                task.control.pause_requested.store(false, Ordering::SeqCst);
                signal_active_process(&task.control, "CONT");
                task.updated_at = timestamp();
                (task.snapshot(), None)
            } else {
                task.worker_started = true;
                task.status = TaskStatus::Parsing;
                task.previous_status = TaskStatus::Parsing;
                task.stage = "正在解析网页".to_string();
                task.updated_at = timestamp();
                (
                    task.snapshot(),
                    Some((
                        task.id.clone(),
                        task.input_url.clone(),
                        task.output_directory.clone(),
                        task.output_file_name.clone(),
                        Arc::clone(&task.control),
                        task.settings.clone(),
                    )),
                )
            };
            registry.running.insert(task_id);
            starts.push(start);
        }
        persist_task_registry(&mut registry)?;
    }

    for (snapshot, maybe_start) in starts {
        emit_task_update(&app, snapshot)?;
        if let Some((task_id, input_url, output_directory, output_file_name, control, settings)) =
            maybe_start
        {
            let app_for_thread = app.clone();
            std::thread::spawn(move || {
                let ctx = TaskContext {
                    app: app_for_thread.clone(),
                    task_id: task_id.clone(),
                    control,
                    settings,
                };
                let result = start_download_blocking(
                    ctx.clone(),
                    input_url,
                    output_directory,
                    output_file_name,
                );
                finish_task(app_for_thread, ctx, result);
            });
        }
    }

    Ok(())
}

fn finish_task(app: tauri::AppHandle, ctx: TaskContext, result: Result<DownloadResult, String>) {
    let (snapshot, event) = {
        let state = app.state::<AppState>();
        let mut registry = match state.tasks.lock() {
            Ok(registry) => registry,
            Err(error) => {
                let _ = app.emit(
                    "download-failed",
                    ErrorPayload {
                        task_id: ctx.task_id.clone(),
                        message: error.to_string(),
                    },
                );
                return;
            }
        };
        registry.running.remove(&ctx.task_id);
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return;
        };
        let finished_at = timestamp();
        let event = match result {
            Ok(mut result) => {
                result.task_id = ctx.task_id.clone();
                task.status = TaskStatus::Completed;
                task.stage = "下载完成".to_string();
                task.output_path = Some(result.output_path.clone());
                task.working_directory = if result.working_directory.is_empty() {
                    None
                } else {
                    Some(result.working_directory.clone())
                };
                task.error_summary = None;
                task.completed_at = Some(finished_at.clone());
                FinishEvent::Completed(result)
            }
            Err(_message) if ctx.control.cancel_requested.load(Ordering::SeqCst) => {
                task.status = TaskStatus::Cancelled;
                task.stage = "已取消".to_string();
                task.error_summary = Some("下载已取消。".to_string());
                task.completed_at = None;
                FinishEvent::Failed(ErrorPayload {
                    task_id: ctx.task_id.clone(),
                    message: "下载已取消。".to_string(),
                })
            }
            Err(message) => {
                task.status = TaskStatus::Failed;
                task.stage = "下载失败".to_string();
                task.error_summary = Some(message.clone());
                task.completed_at = None;
                FinishEvent::Failed(ErrorPayload {
                    task_id: ctx.task_id.clone(),
                    message,
                })
            }
        };
        task.updated_at = finished_at;
        let snapshot = task.snapshot();
        if let Err(error) = persist_task_registry(&mut registry) {
            let _ = app.emit(
                "download-log",
                LogPayload {
                    task_id: ctx.task_id.clone(),
                    message: format!("保存任务历史失败：{error}\n"),
                },
            );
        }
        (snapshot, event)
    };

    let _ = emit_task_update(&app, snapshot);
    match event {
        FinishEvent::Completed(result) => {
            let _ = app.emit("download-completed", result);
        }
        FinishEvent::Failed(payload) => {
            let _ = app.emit("download-failed", payload);
        }
    }
    let _ = schedule_tasks(app);
}

enum FinishEvent {
    Completed(DownloadResult),
    Failed(ErrorPayload),
}

fn is_active_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Parsing
            | TaskStatus::FetchingM3u8
            | TaskStatus::Downloading
            | TaskStatus::Decrypting
            | TaskStatus::Merging
            | TaskStatus::Verifying
    )
}

fn is_terminal_status(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    )
}

fn status_stage(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "队列中",
        TaskStatus::Parsing => "正在解析网页",
        TaskStatus::FetchingM3u8 => "正在获取 m3u8",
        TaskStatus::Downloading => "正在下载分片",
        TaskStatus::Decrypting => "正在解密分片",
        TaskStatus::Merging => "正在合并 mp4",
        TaskStatus::Verifying => "正在验证输出文件",
        TaskStatus::Paused => "已暂停",
        TaskStatus::Completed => "下载完成",
        TaskStatus::Failed => "下载失败",
        TaskStatus::Cancelled => "已取消",
    }
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn load_task_registry() -> Result<TaskRegistry, String> {
    let path = task_history_path();
    if !path.exists() {
        return Ok(TaskRegistry::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("读取任务历史失败：{}，{}", path.display(), error))?;
    if let Ok(index) = serde_json::from_str::<PersistedTaskIndex>(&text) {
        return load_indexed_task_registry(index);
    }
    let persisted = serde_json::from_str::<Vec<PersistedTaskRecord>>(&text)
        .map_err(|error| format!("解析任务历史失败：{}，{}", path.display(), error))?;
    let now = timestamp();
    let mut registry = TaskRegistry::default();
    for item in persisted {
        let metadata = PersistedTaskMetadata::from(&item);
        registry.tasks.insert(
            metadata.id.clone(),
            task_from_metadata(metadata, item.logs, &now),
        );
    }
    Ok(registry)
}

fn load_indexed_task_registry(index: PersistedTaskIndex) -> Result<TaskRegistry, String> {
    let now = timestamp();
    let mut registry = TaskRegistry::default();
    for task_id in index.task_ids {
        let metadata_path = task_metadata_path(&task_id);
        let text = fs::read_to_string(&metadata_path)
            .map_err(|error| format!("读取任务记录失败：{}，{}", metadata_path.display(), error))?;
        let metadata = serde_json::from_str::<PersistedTaskMetadata>(&text)
            .map_err(|error| format!("解析任务记录失败：{}，{}", metadata_path.display(), error))?;
        let logs = read_persisted_task_logs(&task_id)?;
        registry.tasks.insert(
            metadata.id.clone(),
            task_from_metadata(metadata, logs, &now),
        );
    }
    Ok(registry)
}

fn task_from_metadata(
    metadata: PersistedTaskMetadata,
    raw_logs: Vec<String>,
    now: &str,
) -> TaskRecord {
    let was_unfinished = !is_terminal_status(&metadata.status);
    let status = if was_unfinished {
        TaskStatus::Failed
    } else {
        metadata.status
    };
    let stage = if was_unfinished {
        "任务已中断".to_string()
    } else {
        metadata.stage
    };
    let error_summary = if was_unfinished {
        Some("应用已关闭，任务已中断，可重试。".to_string())
    } else {
        metadata.error_summary
    };
    let completed_at = if was_unfinished {
        None
    } else if matches!(status, TaskStatus::Completed) {
        metadata
            .completed_at
            .or_else(|| Some(metadata.updated_at.clone()))
    } else {
        metadata.completed_at
    };
    let logs_len = raw_logs.len();
    let mut logs = VecDeque::with_capacity(MAX_TASK_LOG_LINES.min(logs_len + 1));
    for line in raw_logs
        .into_iter()
        .skip(logs_len.saturating_sub(MAX_TASK_LOG_LINES))
    {
        logs.push_back(line);
    }
    if was_unfinished {
        logs.push_back("应用已关闭，任务已中断，可重试。".to_string());
    }
    TaskRecord {
        id: metadata.id,
        input_url: metadata.input_url,
        output_directory: metadata.output_directory,
        output_file_name: metadata.output_file_name,
        status,
        previous_status: if was_unfinished {
            TaskStatus::Queued
        } else {
            metadata.previous_status
        },
        stage,
        progress_completed: metadata.progress_completed,
        progress_total: metadata.progress_total,
        error_summary,
        output_path: metadata.output_path,
        working_directory: metadata.working_directory,
        logs,
        control: Arc::new(TaskControl::default()),
        worker_started: false,
        created_at: metadata.created_at,
        updated_at: if was_unfinished {
            now.to_string()
        } else {
            metadata.updated_at
        },
        completed_at,
        settings: metadata.settings,
    }
}

fn read_persisted_task_logs(task_id: &str) -> Result<Vec<String>, String> {
    let path = task_log_path(task_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("读取任务日志失败：{}，{}", path.display(), error))?;
    Ok(text.lines().map(ToString::to_string).collect())
}

fn cleanup_persisted_task_files(task_id: &str) {
    let _ = fs::remove_file(task_metadata_path(task_id));
    let _ = fs::remove_file(task_log_path(task_id));
}

fn persist_task_registry(registry: &mut TaskRegistry) -> Result<(), String> {
    let path = task_history_path();
    let Some(parent) = path.parent() else {
        return Err("任务历史路径无效。".to_string());
    };
    fs::create_dir_all(parent).map_err(|error| format!("创建应用数据目录失败：{error}"))?;
    fs::create_dir_all(task_metadata_parent_dir())
        .map_err(|error| format!("创建任务记录目录失败：{error}"))?;
    fs::create_dir_all(task_log_parent_dir())
        .map_err(|error| format!("创建任务日志目录失败：{error}"))?;
    let mut records = registry
        .tasks
        .values()
        .map(PersistedTaskMetadata::from)
        .collect::<Vec<_>>();
    records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let index = PersistedTaskIndex {
        version: 2,
        task_ids: records.iter().map(|record| record.id.clone()).collect(),
    };
    for record in records {
        let metadata_path = task_metadata_path(&record.id);
        let text = serde_json::to_string_pretty(&record)
            .map_err(|error| format!("序列化任务记录失败：{error}"))?;
        fs::write(&metadata_path, text)
            .map_err(|error| format!("写入任务记录失败：{}，{}", metadata_path.display(), error))?;
        if let Some(task) = registry.tasks.get(&record.id) {
            let log_path = task_log_path(&record.id);
            let logs = task.logs.iter().cloned().collect::<Vec<_>>().join("\n");
            fs::write(&log_path, logs)
                .map_err(|error| format!("写入任务日志失败：{}，{}", log_path.display(), error))?;
        }
    }
    let text = serde_json::to_string_pretty(&index)
        .map_err(|error| format!("序列化任务索引失败：{error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("写入任务索引失败：{}，{}", path.display(), error))?;
    registry.last_persisted_at = Some(Instant::now());
    Ok(())
}

fn persist_task_registry_throttled(registry: &mut TaskRegistry) -> Result<(), String> {
    let should_persist = registry
        .last_persisted_at
        .map_or(true, |last| last.elapsed() >= TASK_HISTORY_PERSIST_INTERVAL);
    if should_persist {
        persist_task_registry(registry)?;
    }
    Ok(())
}

fn next_task_sequence(registry: &TaskRegistry) -> u64 {
    registry
        .tasks
        .keys()
        .filter_map(|id| id.strip_prefix("task-"))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

impl From<&TaskRecord> for PersistedTaskRecord {
    fn from(task: &TaskRecord) -> Self {
        Self {
            id: task.id.clone(),
            input_url: task.input_url.clone(),
            output_directory: task.output_directory.clone(),
            output_file_name: task.output_file_name.clone(),
            status: task.status.clone(),
            previous_status: task.previous_status.clone(),
            stage: task.stage.clone(),
            progress_completed: task.progress_completed,
            progress_total: task.progress_total,
            error_summary: task.error_summary.clone(),
            output_path: task.output_path.clone(),
            working_directory: task.working_directory.clone(),
            logs: task.logs.iter().cloned().collect(),
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
            completed_at: task.completed_at.clone(),
            settings: task.settings.clone(),
        }
    }
}

impl From<&PersistedTaskRecord> for PersistedTaskMetadata {
    fn from(task: &PersistedTaskRecord) -> Self {
        Self {
            id: task.id.clone(),
            input_url: task.input_url.clone(),
            output_directory: task.output_directory.clone(),
            output_file_name: task.output_file_name.clone(),
            status: task.status.clone(),
            previous_status: task.previous_status.clone(),
            stage: task.stage.clone(),
            progress_completed: task.progress_completed,
            progress_total: task.progress_total,
            error_summary: task.error_summary.clone(),
            output_path: task.output_path.clone(),
            working_directory: task.working_directory.clone(),
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
            completed_at: task.completed_at.clone(),
            settings: task.settings.clone(),
        }
    }
}

impl From<&TaskRecord> for PersistedTaskMetadata {
    fn from(task: &TaskRecord) -> Self {
        Self {
            id: task.id.clone(),
            input_url: task.input_url.clone(),
            output_directory: task.output_directory.clone(),
            output_file_name: task.output_file_name.clone(),
            status: task.status.clone(),
            previous_status: task.previous_status.clone(),
            stage: task.stage.clone(),
            progress_completed: task.progress_completed,
            progress_total: task.progress_total,
            error_summary: task.error_summary.clone(),
            output_path: task.output_path.clone(),
            working_directory: task.working_directory.clone(),
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
            completed_at: task.completed_at.clone(),
            settings: task.settings.clone(),
        }
    }
}

fn check_tools(settings: &AppSettings) -> Result<Tools, String> {
    Ok(Tools {
        ffmpeg: find_tool_with_override("ffmpeg", settings.ffmpeg_path.as_deref())?,
        ffprobe: find_tool_with_override("ffprobe", settings.ffprobe_path.as_deref())?,
        aria2c: find_tool_with_override("aria2c", settings.aria2c_path.as_deref())?,
    })
}

fn tool_check_item(name: &str, override_path: Option<&str>) -> ToolCheckItem {
    match find_tool_with_override(name, override_path) {
        Ok(path) => ToolCheckItem {
            name: name.to_string(),
            path: Some(path.to_string_lossy().to_string()),
            ok: true,
            message: "可用".to_string(),
        },
        Err(message) => ToolCheckItem {
            name: name.to_string(),
            path: override_path.map(str::to_string),
            ok: false,
            message,
        },
    }
}

fn find_tool_with_override(name: &str, override_path: Option<&str>) -> Result<PathBuf, String> {
    if let Some(path) = override_path.map(str::trim).filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!("配置的 {name} 路径不存在：{}", path.display()));
    }
    find_tool(name)
}

fn find_tool(name: &str) -> Result<PathBuf, String> {
    for candidate in [
        format!("/usr/local/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/bin/{name}"),
        format!("/bin/{name}"),
    ] {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Ok(path);
        }
    }

    let output = Command::new("/usr/bin/which")
        .arg(name)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    Err(format!(
        "未找到 {name}，请先安装：brew install ffmpeg aria2"
    ))
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(BROWSER_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

fn extract_m3u8_url(ctx: &TaskContext, page_url: &Url) -> Result<ExtractedM3u8, String> {
    emit_stage(ctx, TaskStatus::Parsing, "正在解析网页")?;
    emit_log(ctx, &format!("网页 URL: {page_url}\n"))?;
    let page_headers = RequestHeaders {
        referer: Some(page_url.as_str().to_string()),
        origin: page_origin(page_url),
        ..RequestHeaders::default()
    };

    emit_stage(ctx, TaskStatus::Parsing, "正在静态扫描")?;
    for attempt in 1..=3 {
        check_control(ctx)?;
        emit_log(ctx, &format!("静态扫描尝试 {attempt}/3。\n"))?;
        match static_html_extract(ctx, page_url) {
            Ok(candidates) => {
                if let Some(extracted) = validate_url_candidates(ctx, &candidates, &page_headers)? {
                    emit_log(ctx, &format!("静态扫描找到 m3u8: {}\n", extracted.url))?;
                    return Ok(extracted);
                }
                emit_log(ctx, "本次静态扫描未找到可验证的 m3u8。\n")?;
            }
            Err(error) => {
                emit_log(ctx, &format!("本次静态扫描失败：{error}\n"))?;
                if error.contains("404 Not Found") {
                    emit_log(ctx, "网页静态请求返回 404，跳过后续静态重试。\n")?;
                    break;
                }
            }
        };
        if attempt < 3 {
            emit_log(ctx, "等待 1 秒后重试静态扫描。\n")?;
            sleep_with_control(ctx, Duration::from_secs(1))?;
        }
    }
    emit_log(ctx, "静态扫描未找到可验证的 m3u8，准备捕获网络请求。\n")?;

    check_control(ctx)?;
    emit_stage(ctx, TaskStatus::Parsing, "正在捕获网络请求")?;
    if let Some(extracted) = browser_network_extract(ctx, page_url, &page_headers)? {
        emit_log(ctx, &format!("网络捕获找到 m3u8: {}\n", extracted.url))?;
        return Ok(extracted);
    }

    Err(
        "没有从网页中提取到可用 m3u8。若页面需要登录，请在弹出的提取浏览器中登录后重试。"
            .to_string(),
    )
}

fn static_html_extract(ctx: &TaskContext, page_url: &Url) -> Result<Vec<Url>, String> {
    emit_log(ctx, &format!("开始请求网页 HTML: {page_url}\n"))?;
    let response = http_client()?
        .get(page_url.as_str())
        .send()
        .map_err(|error| format!("网页请求失败：{error}"))?;
    let status = response.status();
    emit_log(ctx, &format!("网页 HTTP 状态码: {status}\n"))?;
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(format!(
            "网页请求失败，HTTP 状态码：{}，响应摘要：{}",
            status,
            response_preview(&text)
        ));
    }
    let html = response
        .text()
        .map_err(|error| format!("读取网页内容失败：{error}"))?;
    emit_log(
        ctx,
        &format!("静态 HTML 响应长度: {} 字符。\n", html.chars().count()),
    )?;
    let candidates = collect_m3u8_candidates(page_url, &html);
    emit_log(
        ctx,
        &format!("静态扫描发现 {} 个候选地址。\n", candidates.len()),
    )?;
    log_candidates(ctx, "静态候选", &candidates)?;
    Ok(candidates)
}

fn browser_network_extract(
    ctx: &TaskContext,
    page_url: &Url,
    fallback_headers: &RequestHeaders,
) -> Result<Option<ExtractedM3u8>, String> {
    let mut first_unverified_candidate = None;

    if should_try_persistent_profile(page_url) {
        let persistent_profile = browser_profile_dir();
        let persistent_candidates = run_browser_network_extract(
            ctx,
            page_url,
            &persistent_profile,
            "持久 Profile",
            "abort",
        )?;
        if first_unverified_candidate.is_none() {
            first_unverified_candidate =
                first_direct_candidate(&persistent_candidates, fallback_headers);
        }
        if let Some(extracted) =
            validate_browser_candidates(ctx, &persistent_candidates, fallback_headers)?
        {
            return Ok(Some(extracted));
        }
        emit_log(
            ctx,
            "持久 Profile 未捕获到可验证的 m3u8，改用全新临时 Profile 多轮重试。\n",
        )?;
    } else {
        emit_log(ctx, "该站点优先使用全新临时 Profile，跳过持久 Profile。\n")?;
    }

    for attempt in 1..=3 {
        emit_log(ctx, &format!("全新临时 Profile 尝试 {attempt}/3。\n"))?;
        let fresh_profile = env::temp_dir().join(format!(
            "streamweave-browser-profile-{}-{}-{}",
            Local::now().format("%Y%m%d%H%M%S"),
            stable_hash(page_url.as_str()),
            attempt
        ));
        let profile_label = format!("全新临时 Profile {attempt}/3");
        let fresh_result =
            run_browser_network_extract(ctx, page_url, &fresh_profile, &profile_label, "abort");
        let _ = fs::remove_dir_all(&fresh_profile);
        match fresh_result {
            Ok(candidates) => {
                if should_direct_download_first_candidate(page_url) {
                    if let Some(first_candidate) = candidates.first() {
                        emit_log(
                            ctx,
                            "短 token 站点已提取到 m3u8，立即请求第一个候选 playlist。\n",
                        )?;
                        if let Some(extracted) = validate_browser_candidates(
                            ctx,
                            std::slice::from_ref(first_candidate),
                            fallback_headers,
                        )? {
                            return Ok(Some(extracted));
                        }
                        emit_log(ctx, "本次短 token m3u8 不可用，立即换新 Profile。\n")?;
                        if first_unverified_candidate.is_none() {
                            first_unverified_candidate =
                                first_direct_candidate(&candidates, fallback_headers);
                        }
                        continue;
                    }
                }
                if first_unverified_candidate.is_none() {
                    first_unverified_candidate =
                        first_direct_candidate(&candidates, fallback_headers);
                }
                if let Some(extracted) =
                    validate_browser_candidates(ctx, &candidates, fallback_headers)?
                {
                    return Ok(Some(extracted));
                }
                emit_log(
                    ctx,
                    &format!("全新临时 Profile 尝试 {attempt}/3 未得到可验证 m3u8。\n"),
                )?;
            }
            Err(error) => {
                emit_log(
                    ctx,
                    &format!("全新临时 Profile 尝试 {attempt}/3 捕获失败：{error}\n"),
                )?;
            }
        }
    }

    if should_direct_download_first_candidate(page_url) && first_unverified_candidate.is_some() {
        emit_log(
            ctx,
            "中止浏览器 m3u8 请求模式未成功，改用浏览器响应正文模式重试。\n",
        )?;
        for attempt in 1..=3 {
            emit_log(ctx, &format!("浏览器响应正文模式尝试 {attempt}/3。\n"))?;
            let fresh_profile = env::temp_dir().join(format!(
                "streamweave-browser-profile-{}-{}-allow-{}",
                Local::now().format("%Y%m%d%H%M%S"),
                stable_hash(page_url.as_str()),
                attempt
            ));
            let profile_label = format!("浏览器响应正文模式 {attempt}/3");
            let fresh_result =
                run_browser_network_extract(ctx, page_url, &fresh_profile, &profile_label, "allow");
            let _ = fs::remove_dir_all(&fresh_profile);
            match fresh_result {
                Ok(candidates) => {
                    if let Some(extracted) =
                        validate_browser_candidates(ctx, &candidates, fallback_headers)?
                    {
                        return Ok(Some(extracted));
                    }
                    emit_log(
                        ctx,
                        &format!("浏览器响应正文模式尝试 {attempt}/3 未得到可用 playlist。\n"),
                    )?;
                }
                Err(error) => {
                    emit_log(
                        ctx,
                        &format!("浏览器响应正文模式尝试 {attempt}/3 捕获失败：{error}\n"),
                    )?;
                }
            }
        }
        return Err(
            "已提取到 m3u8 候选，但服务端对所有短 token 候选都返回 404 或无效 playlist。"
                .to_string(),
        );
    }

    if let Some(extracted) = first_unverified_candidate {
        emit_log(
            ctx,
            "已提取到 m3u8 候选但校验失败，继续使用第一个候选直接尝试下载。\n",
        )?;
        return Ok(Some(extracted));
    }

    Ok(None)
}

fn run_browser_network_extract(
    ctx: &TaskContext,
    page_url: &Url,
    profile_dir: &Path,
    profile_label: &str,
    capture_mode: &str,
) -> Result<Vec<BrowserCandidate>, String> {
    let node = find_tool_with_override("node", ctx.settings.node_path.as_deref())
        .map_err(|_| "未找到 node，网页动态提取需要先安装 Node.js。".to_string())?;
    let script = extractor_script_path(ctx)?;
    let browser_mode = BrowserLaunchMode::from_settings(&ctx.settings);
    fs::create_dir_all(&profile_dir).map_err(|error| error.to_string())?;

    emit_log(ctx, &format!("网页提取脚本: {}\n", script.display()))?;
    emit_log(
        ctx,
        &format!(
            "启动 Playwright 捕获网络请求（{profile_label}，{}），浏览器 Profile: {}\n",
            browser_mode.label(),
            profile_dir.display()
        ),
    )?;

    emit_log(
        ctx,
        &format!(
            "{}，将最多尝试 3 轮加载，每轮等待 15 秒。\n",
            browser_mode.status()
        ),
    )?;

    check_control(ctx)?;
    let mut child = Command::new(node)
        .arg(script)
        .arg(page_url.as_str())
        .arg(profile_dir)
        .arg("15000")
        .arg("3")
        .arg(capture_mode)
        .arg(browser_mode.script_arg())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动网页提取脚本失败：{error}"))?;
    register_active_pid(ctx, Some(child.id()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取网页提取脚本输出。".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取网页提取脚本日志。".to_string())?;
    let (tx, rx) = mpsc::channel::<(String, String)>();
    spawn_named_pipe_reader("stdout", stdout, tx.clone());
    spawn_named_pipe_reader("stderr", stderr, tx.clone());
    drop(tx);

    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    loop {
        if let Err(error) = check_control(ctx) {
            let _ = child.kill();
            let _ = child.wait();
            register_active_pid(ctx, None)?;
            return Err(error);
        }
        while let Ok((kind, line)) = rx.try_recv() {
            if kind == "stdout" {
                stdout_text.push_str(&line);
            } else {
                stderr_text.push_str(&line);
                emit_log(ctx, &line)?;
            }
        }

        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            register_active_pid(ctx, None)?;
            while let Ok((kind, line)) = rx.try_recv() {
                if kind == "stdout" {
                    stdout_text.push_str(&line);
                } else {
                    stderr_text.push_str(&line);
                    emit_log(ctx, &line)?;
                }
            }
            if !status.success() {
                return Err(format!("网页网络捕获失败。{}", command_tail(&stderr_text)));
            }
            break;
        }

        sleep_with_control(ctx, std::time::Duration::from_millis(100))?;
    }

    let value: serde_json::Value = serde_json::from_str(stdout_text.trim())
        .map_err(|error| format!("解析网页提取结果失败：{error}"))?;
    let Some(items) = value.get("candidates").and_then(|value| value.as_array()) else {
        return Err("网页提取脚本没有返回候选地址。".to_string());
    };

    let mut candidates = Vec::new();
    for item in items {
        if let Some(candidate) = parse_browser_candidate(page_url, item) {
            push_unique_browser_candidate(&mut candidates, candidate);
        }
    }
    emit_log(
        ctx,
        &format!(
            "{profile_label} 捕获发现 {} 个候选地址。\n",
            candidates.len()
        ),
    )?;
    log_browser_candidates(ctx, &format!("{profile_label} 网络候选"), &candidates)?;
    Ok(candidates)
}

fn collect_m3u8_candidates(base_url: &Url, text: &str) -> Vec<Url> {
    let normalized = normalize_candidate_text(text);
    let lower = normalized.to_lowercase();
    let mut candidates = Vec::new();
    let mut offset = 0usize;

    while let Some(position) = lower[offset..].find(".m3u8") {
        let index = offset + position;
        let start = find_candidate_start(&normalized, index);
        let end = find_candidate_end(&normalized, index + ".m3u8".len());
        let raw = normalized[start..end]
            .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}'));
        if let Ok(url) = base_url.join(raw) {
            push_unique_url(&mut candidates, url);
        }
        offset = end.min(lower.len());
    }

    candidates
}

fn normalize_candidate_text(text: &str) -> String {
    text.replace("\\/", "/")
        .replace("\\u0026", "&")
        .replace("\\u003d", "=")
        .replace("\\u003f", "?")
        .replace("&amp;", "&")
}

fn find_candidate_start(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = index;
    while cursor > 0 {
        let ch = bytes[cursor - 1] as char;
        if ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>' | ',' | ';') {
            break;
        }
        cursor -= 1;
    }
    cursor
}

fn find_candidate_end(text: &str, index: usize) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = index;
    while cursor < bytes.len() {
        let ch = bytes[cursor] as char;
        if ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>' | ',' | ';') {
            break;
        }
        cursor += 1;
    }
    cursor
}

fn validate_url_candidates(
    ctx: &TaskContext,
    candidates: &[Url],
    headers: &RequestHeaders,
) -> Result<Option<ExtractedM3u8>, String> {
    for candidate in candidates {
        check_control(ctx)?;
        emit_log(ctx, &format!("校验候选 m3u8: {candidate}\n"))?;
        log_request_headers(ctx, headers)?;
        match fetch_playlist_text(candidate, headers) {
            Ok(text) if playlist_kind(&text).is_some() => {
                emit_log(ctx, "候选校验通过。\n")?;
                return Ok(Some(ExtractedM3u8 {
                    url: candidate.clone(),
                    headers: headers.clone(),
                    playlist_text: Some(text),
                    suggested_name: None,
                }));
            }
            Ok(text) => emit_log(
                ctx,
                &format!("候选不是有效 HLS，响应摘要: {}\n", response_preview(&text)),
            )?,
            Err(error) => emit_log(ctx, &format!("候选校验失败: {candidate}，{error}\n"))?,
        }
    }
    Ok(None)
}

fn validate_browser_candidates(
    ctx: &TaskContext,
    candidates: &[BrowserCandidate],
    fallback_headers: &RequestHeaders,
) -> Result<Option<ExtractedM3u8>, String> {
    for candidate in candidates {
        check_control(ctx)?;
        let headers = merge_headers(&candidate.headers, fallback_headers);
        emit_log(ctx, &format!("校验候选 m3u8: {}\n", candidate.url))?;
        log_request_headers(ctx, &headers)?;
        if let Some(text) = &candidate.playlist_text {
            if playlist_kind(text).is_some() {
                emit_log(ctx, "候选已带浏览器响应正文，校验通过。\n")?;
                return Ok(Some(ExtractedM3u8 {
                    url: candidate.url.clone(),
                    headers,
                    playlist_text: Some(text.clone()),
                    suggested_name: candidate.suggested_name.clone(),
                }));
            }
            emit_log(
                ctx,
                &format!(
                    "候选浏览器响应不是有效 HLS，响应摘要: {}\n",
                    response_preview(text)
                ),
            )?;
        }
        match fetch_playlist_text(&candidate.url, &headers) {
            Ok(text) if playlist_kind(&text).is_some() => {
                emit_log(ctx, "候选校验通过。\n")?;
                return Ok(Some(ExtractedM3u8 {
                    url: candidate.url.clone(),
                    headers,
                    playlist_text: Some(text),
                    suggested_name: candidate.suggested_name.clone(),
                }));
            }
            Ok(text) => emit_log(
                ctx,
                &format!("候选不是有效 HLS，响应摘要: {}\n", response_preview(&text)),
            )?,
            Err(error) => emit_log(ctx, &format!("候选校验失败: {}，{error}\n", candidate.url))?,
        }
    }
    Ok(None)
}

fn first_direct_candidate(
    candidates: &[BrowserCandidate],
    fallback_headers: &RequestHeaders,
) -> Option<ExtractedM3u8> {
    candidates.first().map(|candidate| ExtractedM3u8 {
        url: candidate.url.clone(),
        headers: merge_headers(&candidate.headers, fallback_headers),
        playlist_text: candidate.playlist_text.clone(),
        suggested_name: candidate.suggested_name.clone(),
    })
}

fn log_candidates(ctx: &TaskContext, label: &str, candidates: &[Url]) -> Result<(), String> {
    for (index, candidate) in candidates.iter().enumerate() {
        emit_log(ctx, &format!("{label} #{}: {candidate}\n", index + 1))?;
    }
    Ok(())
}

fn log_browser_candidates(
    ctx: &TaskContext,
    label: &str,
    candidates: &[BrowserCandidate],
) -> Result<(), String> {
    for (index, candidate) in candidates.iter().enumerate() {
        emit_log(ctx, &format!("{label} #{}: {}\n", index + 1, candidate.url))?;
        log_request_headers(ctx, &candidate.headers)?;
    }
    Ok(())
}

fn push_unique_url(urls: &mut Vec<Url>, url: Url) {
    if !urls.iter().any(|existing| existing == &url) {
        urls.push(url);
    }
}

fn push_unique_browser_candidate(
    candidates: &mut Vec<BrowserCandidate>,
    candidate: BrowserCandidate,
) {
    if !candidates
        .iter()
        .any(|existing| existing.url == candidate.url)
    {
        candidates.push(candidate);
    }
}

fn parse_browser_candidate(page_url: &Url, item: &serde_json::Value) -> Option<BrowserCandidate> {
    if let Some(raw) = item.as_str() {
        return Some(BrowserCandidate {
            url: page_url.join(raw).ok()?,
            headers: RequestHeaders::default(),
            playlist_text: None,
            suggested_name: None,
        });
    }

    let object = item.as_object()?;
    let raw = object.get("url")?.as_str()?;
    Some(BrowserCandidate {
        url: page_url.join(raw).ok()?,
        headers: RequestHeaders {
            referer: object
                .get("referer")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            origin: object
                .get("origin")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            cookie: object
                .get("cookie")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            user_agent: object
                .get("userAgent")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            accept: object
                .get("accept")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            accept_language: object
                .get("acceptLanguage")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        },
        playlist_text: object
            .get("playlistText")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        suggested_name: object
            .get("pageTitle")
            .and_then(|value| value.as_str())
            .and_then(sanitize_file_stem),
    })
}

fn merge_headers(primary: &RequestHeaders, fallback: &RequestHeaders) -> RequestHeaders {
    if primary.referer.is_some() || primary.origin.is_some() {
        return primary.clone();
    }

    RequestHeaders {
        referer: primary.referer.clone().or_else(|| fallback.referer.clone()),
        origin: primary.origin.clone().or_else(|| fallback.origin.clone()),
        cookie: primary.cookie.clone().or_else(|| fallback.cookie.clone()),
        user_agent: primary
            .user_agent
            .clone()
            .or_else(|| fallback.user_agent.clone()),
        accept: primary.accept.clone().or_else(|| fallback.accept.clone()),
        accept_language: primary
            .accept_language
            .clone()
            .or_else(|| fallback.accept_language.clone()),
    }
}

fn should_try_persistent_profile(page_url: &Url) -> bool {
    !is_short_token_page(page_url)
}

fn should_direct_download_first_candidate(page_url: &Url) -> bool {
    is_short_token_page(page_url)
}

fn is_short_token_page(page_url: &Url) -> bool {
    let Some(host) = page_url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    matches!(host.as_str(), "nvhai86.top" | "haouu8.top")
        || (host.starts_with("pochu") && host.ends_with(".top"))
}

fn inferred_direct_m3u8_headers(input: &Url) -> RequestHeaders {
    match input.host_str().map(|host| host.to_ascii_lowercase()) {
        Some(host) if host == "lldvideo.top" || host.ends_with(".lldvideo.top") => RequestHeaders {
            referer: Some("https://haouu8.top/".to_string()),
            user_agent: Some(BROWSER_USER_AGENT.to_string()),
            ..RequestHeaders::default()
        },
        _ => RequestHeaders::default(),
    }
}

fn page_origin(url: &Url) -> Option<String> {
    Some(format!("{}://{}", url.scheme(), url.host_str()?))
}

fn log_request_headers(ctx: &TaskContext, headers: &RequestHeaders) -> Result<(), String> {
    if headers.referer.is_none()
        && headers.origin.is_none()
        && headers.cookie.is_none()
        && headers.user_agent.is_none()
    {
        return Ok(());
    }
    emit_log(
        ctx,
        &format!(
            "使用请求头: Referer={} Origin={} Cookie={} UA={}\n",
            headers.referer.as_deref().unwrap_or("<无>"),
            headers.origin.as_deref().unwrap_or("<无>"),
            if headers.cookie.is_some() {
                "<有>"
            } else {
                "<无>"
            },
            if headers.user_agent.is_some() {
                "<有>"
            } else {
                "<无>"
            }
        ),
    )
}

fn resolve_m3u8(
    input: &Url,
    working_dir: &Path,
    headers: &RequestHeaders,
    playlist_text: Option<String>,
) -> Result<Manifest, String> {
    let (playlist_url, text) = fetch_media_playlist(input, headers, playlist_text)?;

    fs::write(working_dir.join("source.m3u8"), &text).map_err(|error| error.to_string())?;

    let mut segment_entries = Vec::new();
    let mut full_lines = Vec::new();
    let mut next_sequence = 0u64;
    let mut current_key: Option<SegmentEncryption> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if let Some(value) = trimmed.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                next_sequence = value.trim().parse().unwrap_or(0);
            } else if let Some(key) = parse_segment_key(trimmed, &playlist_url, headers)? {
                current_key = Some(key);
            }
            full_lines.push(line.to_string());
            continue;
        }

        let url = playlist_url
            .join(trimmed)
            .map_err(|_| format!("无法解析分片 URL：{trimmed}"))?;
        full_lines.push(url.to_string());
        segment_entries.push(SegmentEntry {
            url,
            sequence: next_sequence,
            encryption: current_key.clone(),
        });
        next_sequence = next_sequence.saturating_add(1);
    }

    if segment_entries.is_empty() {
        return Err("m3u8 中没有找到可下载分片。".to_string());
    }

    fs::write(working_dir.join("full.m3u8"), full_lines.join("\n"))
        .map_err(|error| error.to_string())?;

    let mut aria2 = String::new();
    let mut concat = String::new();
    for (index, entry) in segment_entries.iter().enumerate() {
        let name = segment_name(index);
        aria2.push_str(entry.url.as_str());
        aria2.push_str("\n  out=");
        aria2.push_str(&name);
        aria2.push('\n');

        let path = working_dir.join("parts").join(name);
        concat.push_str("file '");
        concat.push_str(&path.to_string_lossy().replace('\'', "'\\''"));
        concat.push_str("'\n");
    }

    fs::write(working_dir.join("aria2.txt"), aria2).map_err(|error| error.to_string())?;
    fs::write(working_dir.join("concat.txt"), concat).map_err(|error| error.to_string())?;

    Ok(Manifest { segment_entries })
}

fn fetch_media_playlist(
    input: &Url,
    headers: &RequestHeaders,
    playlist_text: Option<String>,
) -> Result<(Url, String), String> {
    let text = match playlist_text {
        Some(text) => text,
        None => fetch_playlist_text(input, headers)?,
    };
    match playlist_kind(&text) {
        Some(PlaylistKind::Media) => Ok((input.clone(), text)),
        Some(PlaylistKind::Master) => {
            let variant = select_master_variant(input, &text)?;
            let variant_text = fetch_playlist_text(&variant, headers)?;
            match playlist_kind(&variant_text) {
                Some(PlaylistKind::Media) => Ok((variant, variant_text)),
                _ => {
                    Err("master playlist 中选出的 variant 不是可下载 media playlist。".to_string())
                }
            }
        }
        None => Err(format!(
            "m3u8 无效或 token 已过期，请重新获取链接。响应摘要：{}",
            response_preview(&text)
        )),
    }
}

fn parse_segment_key(
    line: &str,
    playlist_url: &Url,
    headers: &RequestHeaders,
) -> Result<Option<SegmentEncryption>, String> {
    if !line.starts_with("#EXT-X-KEY:") {
        return Ok(None);
    }

    let method = attribute_value(line, "METHOD").unwrap_or("");
    if method == "NONE" {
        return Ok(None);
    }
    if method != "AES-128" {
        return Err(format!("暂不支持的 HLS 加密方式：{method}"));
    }

    let Some(raw_uri) = attribute_value(line, "URI") else {
        return Err("EXT-X-KEY 缺少 URI。".to_string());
    };
    let key_url = playlist_url
        .join(raw_uri)
        .map_err(|_| format!("无法解析加密 key URL：{raw_uri}"))?;
    let key_bytes = fetch_binary_text(&key_url, headers)?;
    if key_bytes.len() != 16 {
        return Err(format!(
            "AES-128 key 长度异常：期望 16 字节，实际 {} 字节",
            key_bytes.len()
        ));
    }

    let iv = attribute_value(line, "IV").map(parse_hls_iv).transpose()?;

    Ok(Some(SegmentEncryption { key_bytes, iv }))
}

fn fetch_playlist_text(url: &Url, headers: &RequestHeaders) -> Result<String, String> {
    fetch_text(url, headers)
}

fn fetch_binary_text(url: &Url, headers: &RequestHeaders) -> Result<Vec<u8>, String> {
    let response = build_request(url, headers)?
        .send()
        .map_err(|error| format!("网络请求失败，请检查链接是否可访问：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(format!(
            "m3u8 请求失败，HTTP 状态码：{}，响应摘要：{}",
            status,
            response_preview(&text)
        ));
    }
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|error| format!("读取二进制内容失败：{error}"))
}

fn fetch_text(url: &Url, headers: &RequestHeaders) -> Result<String, String> {
    let response = build_request(url, headers)?
        .send()
        .map_err(|error| format!("网络请求失败，请检查链接是否可访问：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(format!(
            "m3u8 请求失败，HTTP 状态码：{}，响应摘要：{}",
            status,
            response_preview(&text)
        ));
    }
    response
        .text()
        .map_err(|error| format!("读取 m3u8 内容失败：{error}"))
}

fn build_request(
    url: &Url,
    headers: &RequestHeaders,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    let mut request = http_client()?.get(url.as_str());
    if let Some(referer) = &headers.referer {
        request = request.header("Referer", referer);
    }
    if let Some(origin) = &headers.origin {
        request = request.header("Origin", origin);
    }
    if let Some(cookie) = &headers.cookie {
        request = request.header("Cookie", cookie);
    }
    if let Some(user_agent) = &headers.user_agent {
        request = request.header("User-Agent", user_agent);
    }
    if let Some(accept) = &headers.accept {
        request = request.header("Accept", accept);
    }
    if let Some(accept_language) = &headers.accept_language {
        request = request.header("Accept-Language", accept_language);
    }
    Ok(request)
}

fn response_preview(text: &str) -> String {
    let preview = text
        .chars()
        .take(500)
        .collect::<String>()
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    if preview.is_empty() {
        "<空响应>".to_string()
    } else {
        preview
    }
}

fn playlist_kind(text: &str) -> Option<PlaylistKind> {
    if !text.contains("#EXTM3U") {
        return None;
    }
    if text.contains("#EXTINF") {
        return Some(PlaylistKind::Media);
    }
    if text.contains("#EXT-X-STREAM-INF") {
        return Some(PlaylistKind::Master);
    }
    None
}

fn select_master_variant(master_url: &Url, text: &str) -> Result<Url, String> {
    let mut pending_score = None;
    let mut best: Option<(u64, Url)> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#EXT-X-STREAM-INF") {
            pending_score = Some(stream_inf_score(trimmed));
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(score) = pending_score.take() {
            let url = master_url
                .join(trimmed)
                .map_err(|_| format!("无法解析 master playlist variant：{trimmed}"))?;
            if best
                .as_ref()
                .map_or(true, |(best_score, _)| score > *best_score)
            {
                best = Some((score, url));
            }
        }
    }

    best.map(|(_, url)| url)
        .ok_or_else(|| "master playlist 中没有找到可用 variant。".to_string())
}

fn stream_inf_score(line: &str) -> u64 {
    let bandwidth = attribute_value(line, "BANDWIDTH")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let pixels = attribute_value(line, "RESOLUTION")
        .and_then(|value| {
            value
                .split_once('x')
                .map(|(width, height)| (width.to_string(), height.to_string()))
        })
        .and_then(|(width, height)| Some(width.parse::<u64>().ok()? * height.parse::<u64>().ok()?))
        .unwrap_or(0);
    pixels.saturating_mul(10_000_000).saturating_add(bandwidth)
}

fn attribute_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    let start = line.find(&prefix)? + prefix.len();
    let value = &line[start..];
    let end = value.find(',').unwrap_or(value.len());
    Some(value[..end].trim_matches('"'))
}

fn download_segments(
    ctx: &TaskContext,
    aria2c: &Path,
    working_dir: &Path,
    total: usize,
    headers: &RequestHeaders,
) -> Result<(), String> {
    let parts_dir = working_dir.join("parts");
    let mut completed = 0usize;
    let mut args = vec![
        "--enable-rpc=false".to_string(),
        "-i".to_string(),
        working_dir.join("aria2.txt").to_string_lossy().to_string(),
        "-d".to_string(),
        parts_dir.to_string_lossy().to_string(),
        "-j".to_string(),
        "32".to_string(),
        "-x".to_string(),
        "1".to_string(),
        "-s".to_string(),
        "1".to_string(),
        "-c".to_string(),
        "--max-tries=8".to_string(),
        "--retry-wait=2".to_string(),
        "--summary-interval=5".to_string(),
    ];
    if let Some(referer) = &headers.referer {
        args.push(format!("--header=Referer: {referer}"));
    }
    if let Some(origin) = &headers.origin {
        args.push(format!("--header=Origin: {origin}"));
    }
    if let Some(cookie) = &headers.cookie {
        args.push(format!("--header=Cookie: {cookie}"));
    }
    if let Some(user_agent) = &headers.user_agent {
        args.push(format!("--user-agent={user_agent}"));
    } else {
        args.push(format!("--user-agent={BROWSER_USER_AGENT}"));
    }
    if let Some(accept) = &headers.accept {
        args.push(format!("--header=Accept: {accept}"));
    }
    if let Some(accept_language) = &headers.accept_language {
        args.push(format!("--header=Accept-Language: {accept_language}"));
    }
    args.extend(parse_extra_args(ctx.settings.aria2_args.as_deref())?);

    for attempt in 1..=2 {
        run_command_streaming(ctx, aria2c, args.clone(), false, |line| {
            if line.contains("Download complete:") {
                completed = completed.saturating_add(1).min(total);
                let _ = emit_progress(ctx, completed, total);
            }
        })?;

        match verify_segment_files(&parts_dir, total) {
            Ok(()) => break,
            Err(error) if attempt == 1 => {
                emit_log(
                    ctx,
                    &format!("分片校验失败，准备重试 aria2 一次：{error}\n"),
                )?;
                sleep_with_control(ctx, Duration::from_secs(1))?;
            }
            Err(error) => return Err(error),
        }
    }

    emit_progress(ctx, total, total)?;
    Ok(())
}

fn verify_segment_files(parts_dir: &Path, total: usize) -> Result<(), String> {
    let mut missing = Vec::new();
    for index in 0..total {
        let path = parts_dir.join(segment_name(index));
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > 0 => {}
            Ok(_) => missing.push(format!("{} (空文件)", path.display())),
            Err(_) => missing.push(format!("{} (缺失)", path.display())),
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    Err(format!("分片下载未完成：{}", missing.join("，")))
}

#[derive(Default)]
struct SegmentProbeResult {
    has_media_stream: bool,
    stream_summary: String,
}

fn validate_segments_before_merge(
    ctx: &TaskContext,
    ffprobe: &Path,
    working_dir: &Path,
    total: usize,
) -> Result<(), String> {
    if total == 0 {
        return Err("没有可检查的分片，无法合并。".to_string());
    }

    let sample_indices = segment_probe_sample_indices(total);
    emit_log(
        ctx,
        &format!(
            "合并前检查分片媒体流，抽样 {} / {} 个分片。\n",
            sample_indices.len(),
            total
        ),
    )?;

    let parts_dir = working_dir.join("parts");
    let mut reports = Vec::new();
    let mut valid_reports = Vec::new();
    for index in sample_indices {
        check_control(ctx)?;
        let path = parts_dir.join(segment_name(index));
        let metadata = fs::metadata(&path)
            .map_err(|_| format!("合并前检查失败：分片缺失，{}。", path.display()))?;
        if metadata.len() == 0 {
            return Err(format!("合并前检查失败：分片为空，{}。", path.display()));
        }

        match probe_segment_file(ctx, ffprobe, &path) {
            Ok(probe) if probe.has_media_stream => {
                valid_reports.push(format!(
                    "{}：{}，{}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("<未知分片>"),
                    format_size(metadata.len()),
                    probe.stream_summary
                ));
            }
            Ok(probe) => reports.push(format!(
                "{}：{}，{}，{}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<未知分片>"),
                format_size(metadata.len()),
                probe.stream_summary,
                file_signature_summary(&path)
            )),
            Err(error) => reports.push(format!(
                "{}：{}，ffprobe 失败：{}，{}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<未知分片>"),
                format_size(metadata.len()),
                error,
                file_signature_summary(&path)
            )),
        }
    }

    if reports.is_empty() {
        emit_log(
            ctx,
            &format!("分片媒体流检查通过：{}。\n", valid_reports.join("；")),
        )?;
        return Ok(());
    }

    emit_log(
        ctx,
        &format!("分片媒体流检查未通过：{}。\n", reports.join("；")),
    )?;
    Err(format!(
        "合并前检查失败：抽样分片都没有检测到 video/audio 流。可能是分片未正确解密、下载到了错误响应，或 m3u8 token/请求头已失效。{}",
        reports.join("；")
    ))
}

fn segment_probe_sample_indices(total: usize) -> Vec<usize> {
    let mut indices = vec![
        0,
        total / 4,
        total / 2,
        total.saturating_mul(3) / 4,
        total - 1,
    ];
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn probe_segment_file(
    ctx: &TaskContext,
    ffprobe: &Path,
    path: &Path,
) -> Result<SegmentProbeResult, String> {
    check_control(ctx)?;
    let output = Command::new(ffprobe)
        .args(vec![
            "-hide_banner".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-show_entries".to_string(),
            "stream=codec_type,codec_name".to_string(),
            "-of".to_string(),
            "json".to_string(),
            path.to_string_lossy().to_string(),
        ])
        .output()
        .map_err(|error| error.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(command_tail(&stderr));
    }

    let value: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|error| format!("解析 ffprobe 输出失败：{error}"))?;
    let streams = value
        .get("streams")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut descriptions = Vec::new();
    let mut has_media_stream = false;
    for stream in streams {
        let codec_type = stream
            .get("codec_type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let codec_name = stream
            .get("codec_name")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        if matches!(codec_type, "video" | "audio") {
            has_media_stream = true;
        }
        let description = format!("{codec_type}/{codec_name}");
        if !descriptions.iter().any(|item| item == &description) {
            descriptions.push(description);
        }
    }

    Ok(SegmentProbeResult {
        has_media_stream,
        stream_summary: if descriptions.is_empty() {
            "未检测到任何流".to_string()
        } else {
            format!("流：{}", descriptions.join(", "))
        },
    })
}

fn file_signature_summary(path: &Path) -> String {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) => return format!("文件头读取失败：{error}"),
    };
    if data.is_empty() {
        return "文件头：<空文件>".to_string();
    }

    let prefix = data.iter().take(64).copied().collect::<Vec<_>>();
    let looks_text = prefix
        .iter()
        .all(|byte| byte.is_ascii_graphic() || matches!(*byte, b' ' | b'\n' | b'\r' | b'\t'));
    if looks_text {
        let text = String::from_utf8_lossy(&prefix)
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        return format!("文件头文本：{}", text.chars().take(80).collect::<String>());
    }

    let hex = prefix
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("文件头十六进制：{hex}")
}

fn merge_media(
    ctx: &TaskContext,
    ffmpeg: &Path,
    working_dir: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let mut args = vec![
        "-hide_banner".to_string(),
        "-y".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        working_dir.join("concat.txt").to_string_lossy().to_string(),
        "-c".to_string(),
        "copy".to_string(),
        "-bsf:a".to_string(),
        "aac_adtstoasc".to_string(),
    ];
    args.extend(parse_extra_args(ctx.settings.ffmpeg_args.as_deref())?);
    args.push(output_path.to_string_lossy().to_string());

    run_command_streaming(ctx, ffmpeg, args, true, |_| {})
}

fn validate_output_file(output_path: &Path) -> Result<(), String> {
    let metadata =
        fs::metadata(output_path).map_err(|_| "合并完成后没有找到输出 mp4。".to_string())?;
    if metadata.len() == 0 {
        return Err("合并后的 mp4 文件大小为 0。".to_string());
    }
    Ok(())
}

fn parse_extra_args(raw: Option<&str>) -> Result<Vec<String>, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };

    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(active), value) if value == active => quote = None,
            (Some(_), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), value) => current.push(value),
            (None, '"' | '\'') => quote = Some(ch),
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (None, value) if value.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            (None, value) => current.push(value),
        }
    }

    if quote.is_some() {
        return Err("附加参数中的引号未闭合。".to_string());
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

fn decrypt_segments(
    ctx: &TaskContext,
    working_dir: &Path,
    segments: &[SegmentEntry],
) -> Result<(), String> {
    let encrypted = segments
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, segment)| segment.encryption.is_some())
        .collect::<Vec<_>>();
    if encrypted.is_empty() {
        return Ok(());
    }

    let global_limit = global_decrypt_worker_limit(ctx);
    let worker_count = encrypted.len().min(global_limit);
    emit_log(
        ctx,
        &format!(
            "并行解密 {} 个加密分片，全局解密槽位：{}。\n",
            encrypted.len(),
            global_limit
        ),
    )?;

    let next_job = Arc::new(Mutex::new(0usize));
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_job = Arc::clone(&next_job);
            let task_ctx = ctx.clone();
            let encrypted = &encrypted;
            handles.push(scope.spawn(move || -> Result<(), String> {
                loop {
                    check_control(&task_ctx)?;
                    let job = {
                        let mut next = next_job.lock().map_err(|error| error.to_string())?;
                        if *next >= encrypted.len() {
                            None
                        } else {
                            let job = encrypted[*next].clone();
                            *next += 1;
                            Some(job)
                        }
                    };
                    let Some((index, segment)) = job else {
                        break;
                    };
                    let _permit = acquire_decrypt_permit(&task_ctx)?;
                    decrypt_segment_file(working_dir, index, &segment)?;
                }
                Ok(())
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err("解密线程异常退出。".to_string()),
            }
        }
        Ok(())
    })
}

fn global_decrypt_worker_limit(ctx: &TaskContext) -> usize {
    ctx.app
        .state::<AppState>()
        .max_decrypt_workers
        .load(Ordering::SeqCst)
        .clamp(MIN_DECRYPT_WORKERS as u64, MAX_DECRYPT_WORKERS as u64) as usize
}

fn acquire_decrypt_permit(ctx: &TaskContext) -> Result<DecryptPermit, String> {
    let state = ctx.app.state::<AppState>();
    let limiter = Arc::clone(&state.decrypt_limiter);
    loop {
        check_control(ctx)?;
        let limit = state
            .max_decrypt_workers
            .load(Ordering::SeqCst)
            .clamp(MIN_DECRYPT_WORKERS as u64, MAX_DECRYPT_WORKERS as u64)
            as usize;
        let mut active = limiter.active.lock().map_err(|error| error.to_string())?;
        if *active < limit {
            *active += 1;
            return Ok(DecryptPermit {
                limiter: Arc::clone(&limiter),
            });
        }
        let active_count = *active;
        let should_log = ctx
            .control
            .decrypt_wait_logged
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if should_log {
            drop(active);
            emit_log(
                ctx,
                &format!("全局解密槽位已满，等待可用槽位（{active_count}/{limit} 已占用）。\n"),
            )?;
            continue;
        }
        let (next_active, _) = limiter
            .changed
            .wait_timeout(active, Duration::from_millis(120))
            .map_err(|error| error.to_string())?;
        drop(next_active);
    }
}

fn decrypt_segment_file(
    working_dir: &Path,
    index: usize,
    segment: &SegmentEntry,
) -> Result<(), String> {
    let Some(encryption) = &segment.encryption else {
        return Ok(());
    };
    let path = working_dir.join("parts").join(segment_name(index));
    let mut data = fs::read(&path).map_err(|error| error.to_string())?;
    let iv = encryption
        .iv
        .unwrap_or_else(|| sequence_iv(segment.sequence));
    let key = encryption.key_bytes.as_slice();
    let decryptor = Decryptor::<Aes128>::new_from_slices(key, &iv)
        .map_err(|error| format!("初始化 AES 解密失败：{error}"))?;
    let plaintext = decryptor
        .decrypt_padded_mut::<Pkcs7>(&mut data)
        .map_err(|error| format!("解密分片失败：{}，{error}", path.display()))?;
    fs::write(&path, plaintext).map_err(|error| error.to_string())
}

fn sequence_iv(sequence: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&sequence.to_be_bytes());
    iv
}

fn parse_hls_iv(raw: &str) -> Result<[u8; 16], String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if hex.len() != 32 {
        return Err(format!("HLS IV 长度异常：{trimmed}"));
    }

    let mut iv = [0u8; 16];
    for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
        iv[index] = u8::from_str_radix(pair, 16)
            .map_err(|error| format!("HLS IV 解析失败：{trimmed}，{error}"))?;
    }
    Ok(iv)
}

fn verify_media(
    ctx: &TaskContext,
    ffprobe: &Path,
    output_path: &Path,
) -> Result<ProbeResult, String> {
    check_control(ctx)?;
    let output = Command::new(ffprobe)
        .args(vec![
            "-hide_banner".to_string(),
            "-v".to_string(),
            "error".to_string(),
            "-show_entries".to_string(),
            "format=duration,size".to_string(),
            "-show_entries".to_string(),
            "stream=index,codec_type,codec_name,width,height".to_string(),
            "-of".to_string(),
            "default=noprint_wrappers=1".to_string(),
            output_path.to_string_lossy().to_string(),
        ])
        .output()
        .map_err(|error| error.to_string())?;

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(format!("ffprobe 验证失败。{}", command_tail(&stderr)));
    }

    emit_log(ctx, &text)?;

    let mut result = ProbeResult::default();
    let mut current_codec = String::new();

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "codec_name" => current_codec = value.to_string(),
            "codec_type" if value == "video" => result.video_codec = current_codec.clone(),
            "codec_type" if value == "audio" => result.audio_codec = current_codec.clone(),
            "width" => result.width = value.parse().unwrap_or(0),
            "height" => result.height = value.parse().unwrap_or(0),
            "duration" => result.duration = value.parse().unwrap_or(0.0),
            "size" => result.size = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    if result.duration <= 0.0 || result.size == 0 || result.video_codec.is_empty() {
        return Err("ffprobe 未返回完整媒体信息。".to_string());
    }
    Ok(result)
}

fn cleanup_working_directory(ctx: &TaskContext, working_dir: &Path) {
    match fs::remove_dir_all(working_dir) {
        Ok(()) => {
            let _ = emit_log(ctx, &format!("临时文件已清理: {}\n", working_dir.display()));
        }
        Err(error) => {
            let _ = emit_log(
                ctx,
                &format!(
                    "临时文件清理失败，已保留目录: {}，{}\n",
                    working_dir.display(),
                    error
                ),
            );
        }
    }
}

fn cleanup_task_temporary_files(task: &TaskRecord) -> Result<(), String> {
    let Some(working_directory) = &task.working_directory else {
        return Ok(());
    };
    let path = PathBuf::from(working_directory);
    if !path.exists() {
        return Ok(());
    }

    if !is_task_workspace_dir(&path) {
        return Err(format!(
            "拒绝删除非任务临时目录：{}",
            path.to_string_lossy()
        ));
    }

    fs::remove_dir_all(&path)
        .map_err(|error| format!("删除任务临时文件失败：{}，{}", path.display(), error))?;
    Ok(())
}

fn is_task_workspace_dir(path: &Path) -> bool {
    path.parent().is_some_and(is_task_parent_dir)
}

fn is_task_parent_dir(path: &Path) -> bool {
    if path == task_workspace_parent_dir() {
        return true;
    }
    path.file_name()
        .is_some_and(|name| name == "VideoDownloaderTasks")
}

fn run_command_streaming<F>(
    ctx: &TaskContext,
    executable: &Path,
    args: Vec<String>,
    emit_output: bool,
    mut on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    check_control(ctx)?;

    let mut child = Command::new(executable)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    register_active_pid(ctx, Some(child.id()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取命令输出。".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取命令错误输出。".to_string())?;
    let (tx, rx) = mpsc::channel::<String>();

    spawn_pipe_reader(stdout, tx.clone());
    spawn_pipe_reader(stderr, tx.clone());
    drop(tx);
    let mut recent_output = VecDeque::with_capacity(12);

    loop {
        if let Err(error) = check_control(ctx) {
            let _ = child.kill();
            let _ = child.wait();
            register_active_pid(ctx, None)?;
            let _ = emit_log(ctx, "用户已取消，正在终止当前外部进程。\n");
            return Err(error);
        }

        while let Ok(line) = rx.try_recv() {
            if emit_output {
                emit_log(ctx, &line)?;
            }
            remember_line(&mut recent_output, &line);
            on_line(&line);
        }

        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            register_active_pid(ctx, None)?;
            while let Ok(line) = rx.try_recv() {
                if emit_output {
                    emit_log(ctx, &line)?;
                }
                remember_line(&mut recent_output, &line);
                on_line(&line);
            }
            if status.success() {
                return Ok(());
            }
            return Err(format!(
                "命令执行失败：{}，退出码 {:?}。{}",
                executable.display(),
                status.code(),
                recent_lines_summary(&recent_output)
            ));
        }

        sleep_with_control(ctx, std::time::Duration::from_millis(100))?;
    }
}

fn remember_line(recent_output: &mut VecDeque<String>, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if recent_output.len() == 12 {
        recent_output.pop_front();
    }
    recent_output.push_back(trimmed.to_string());
}

fn recent_lines_summary(recent_output: &VecDeque<String>) -> String {
    if recent_output.is_empty() {
        return "没有捕获到命令输出。".to_string();
    }
    format!(
        "最后输出：{}",
        recent_output
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    )
}

fn command_tail(text: &str) -> String {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(6)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "没有错误输出。".to_string()
    } else {
        lines.reverse();
        format!("错误输出：{}", lines.join(" | "))
    }
}

fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn spawn_pipe_reader<R>(pipe: R, tx: mpsc::Sender<String>)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let _ = tx.send(format!("{line}\n"));
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_named_pipe_reader<R>(name: &'static str, pipe: R, tx: mpsc::Sender<(String, String)>)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let _ = tx.send((name.to_string(), format!("{line}\n")));
                }
                Err(_) => break,
            }
        }
    });
}

fn check_control(ctx: &TaskContext) -> Result<(), String> {
    check_control_state(&ctx.control)
}

fn check_control_state(control: &TaskControl) -> Result<(), String> {
    if control.cancel_requested.load(Ordering::SeqCst) {
        return Err("下载已取消。".to_string());
    }
    while control.pause_requested.load(Ordering::SeqCst) {
        if control.cancel_requested.load(Ordering::SeqCst) {
            return Err("下载已取消。".to_string());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Ok(())
}

fn sleep_with_control(ctx: &TaskContext, duration: Duration) -> Result<(), String> {
    let mut elapsed = Duration::from_millis(0);
    let step = Duration::from_millis(100);
    while elapsed < duration {
        check_control(ctx)?;
        let remaining = duration.saturating_sub(elapsed);
        let current = remaining.min(step);
        std::thread::sleep(current);
        elapsed += current;
    }
    check_control(ctx)
}

fn register_active_pid(ctx: &TaskContext, pid: Option<u32>) -> Result<(), String> {
    *ctx.control
        .active_pid
        .lock()
        .map_err(|error| error.to_string())? = pid;
    Ok(())
}

fn signal_active_process(control: &TaskControl, signal: &str) {
    let pid = match control.active_pid.lock() {
        Ok(pid) => *pid,
        Err(_) => None,
    };
    let Some(pid) = pid else {
        return;
    };
    let _ = Command::new("/bin/kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status();
}

fn emit_log(ctx: &TaskContext, message: &str) -> Result<(), String> {
    {
        let state = ctx.app.state::<AppState>();
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return Ok(());
        };
        for line in message.lines() {
            if !line.trim().is_empty() {
                if task.logs.len() == MAX_TASK_LOG_LINES {
                    task.logs.pop_front();
                }
                task.logs.push_back(line.to_string());
            }
        }
        task.updated_at = timestamp();
        persist_task_registry_throttled(&mut registry)?;
    }
    ctx.app
        .emit(
            "download-log",
            LogPayload {
                task_id: ctx.task_id.clone(),
                message: message.to_string(),
            },
        )
        .map_err(|error| error.to_string())
}

fn emit_stage(ctx: &TaskContext, status: TaskStatus, stage: &str) -> Result<(), String> {
    let snapshot = {
        let state = ctx.app.state::<AppState>();
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return Ok(());
        };
        if task.status != TaskStatus::Paused {
            task.status = status.clone();
            task.previous_status = status.clone();
        }
        task.stage = stage.to_string();
        task.updated_at = timestamp();
        let snapshot = task.snapshot();
        persist_task_registry(&mut registry)?;
        snapshot
    };
    ctx.app
        .emit("download-task-updated", snapshot)
        .map_err(|error| error.to_string())?;
    ctx.app
        .emit(
            "download-stage",
            StagePayload {
                task_id: ctx.task_id.clone(),
                stage: stage.to_string(),
                status,
            },
        )
        .map_err(|error| error.to_string())
}

fn emit_progress(ctx: &TaskContext, completed: usize, total: usize) -> Result<(), String> {
    {
        let state = ctx.app.state::<AppState>();
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return Ok(());
        };
        task.progress_completed = completed;
        task.progress_total = total;
        task.updated_at = timestamp();
        persist_task_registry_throttled(&mut registry)?;
    }
    ctx.app
        .emit(
            "download-progress",
            ProgressPayload {
                task_id: ctx.task_id.clone(),
                completed,
                total,
            },
        )
        .map_err(|error| error.to_string())
}

fn emit_task_directory(ctx: &TaskContext, working_dir: &Path) -> Result<(), String> {
    update_task_paths(ctx, Some(working_dir), None)?;
    ctx.app
        .emit(
            "download-task-directory",
            TaskDirectoryPayload {
                task_id: ctx.task_id.clone(),
                path: working_dir.to_string_lossy().to_string(),
            },
        )
        .map_err(|error| error.to_string())
}

fn update_task_paths(
    ctx: &TaskContext,
    working_dir: Option<&Path>,
    output_path: Option<&Path>,
) -> Result<(), String> {
    let snapshot = {
        let state = ctx.app.state::<AppState>();
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return Ok(());
        };
        if let Some(working_dir) = working_dir {
            task.working_directory = Some(working_dir.to_string_lossy().to_string());
        }
        if let Some(output_path) = output_path {
            task.output_path = Some(output_path.to_string_lossy().to_string());
        }
        task.updated_at = timestamp();
        let snapshot = task.snapshot();
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&ctx.app, snapshot)
}

fn update_task_suggested_name(ctx: &TaskContext, suggested_name: &str) -> Result<(), String> {
    let snapshot = {
        let state = ctx.app.state::<AppState>();
        let mut registry = state.tasks.lock().map_err(|error| error.to_string())?;
        let Some(task) = registry.tasks.get_mut(&ctx.task_id) else {
            return Ok(());
        };
        if task
            .output_file_name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
        {
            task.output_file_name = Some(suggested_name.to_string());
            task.updated_at = timestamp();
        }
        let snapshot = task.snapshot();
        persist_task_registry(&mut registry)?;
        snapshot
    };
    emit_task_update(&ctx.app, snapshot)
}

fn emit_task_update(app: &tauri::AppHandle, snapshot: TaskSnapshot) -> Result<(), String> {
    app.emit("download-task-updated", snapshot)
        .map_err(|error| error.to_string())
}

fn extractor_script_path(ctx: &TaskContext) -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = ctx.app.path().resource_dir() {
        candidates.push(resource_dir.join("scripts/extract-m3u8.mjs"));
        candidates.push(resource_dir.join("extract-m3u8.mjs"));
    }
    candidates.push(cwd.join("scripts/extract-m3u8.mjs"));
    candidates.push(
        cwd.parent()
            .unwrap_or(cwd.as_path())
            .join("scripts/extract-m3u8.mjs"),
    );
    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| "未找到网页提取脚本 scripts/extract-m3u8.mjs。".to_string())
}

fn browser_profile_dir() -> PathBuf {
    app_support_dir().join("browser-profile")
}

fn task_workspace_parent_dir() -> PathBuf {
    app_support_dir().join("tasks")
}

fn task_history_path() -> PathBuf {
    app_support_dir().join("tasks.json")
}

fn task_metadata_parent_dir() -> PathBuf {
    app_support_dir().join("task-records")
}

fn task_log_parent_dir() -> PathBuf {
    app_support_dir().join("task-logs")
}

fn task_metadata_path(task_id: &str) -> PathBuf {
    task_metadata_parent_dir().join(format!("{}.json", safe_task_file_stem(task_id)))
}

fn task_log_path(task_id: &str) -> PathBuf {
    task_log_parent_dir().join(format!("{}.log", safe_task_file_stem(task_id)))
}

fn safe_task_file_stem(task_id: &str) -> String {
    task_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn app_support_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("Application Support")
        .join("StreamWeave")
}

fn default_downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
}

fn output_name(
    input: &Url,
    output_file_name: Option<String>,
    suggested_name: Option<&str>,
) -> String {
    if let Some(name) = output_file_name.as_deref().and_then(sanitize_file_stem) {
        return format!("{name}.mp4");
    }

    if let Some(name) = suggested_name.and_then(sanitize_file_stem) {
        return format!("{name}.mp4");
    }

    let basename = input
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or("video")
        .trim_end_matches(".m3u8");

    if let Some(name) = sanitize_file_stem(basename) {
        format!("{name}.mp4")
    } else {
        format!("video-{}.mp4", Local::now().format("%Y%m%d-%H%M%S"))
    }
}

fn sanitize_file_stem(value: &str) -> Option<String> {
    let mut name = value.trim().trim_end_matches(".mp4").trim().to_string();
    for suffix in [" - 在线观看", " 在线观看", "_在线播放", "在线播放"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim().to_string();
        }
    }

    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let sanitized = sanitized
        .trim_matches(|ch| matches!(ch, '.' | ' '))
        .chars()
        .take(120)
        .collect::<String>();

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

fn segment_name(index: usize) -> String {
    format!("seg{index:05}.ts")
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 1469598103934665603u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn format_duration(duration: f64) -> String {
    let total = duration.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_size(size: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let size = size as f64;
    if size >= GIB {
        format!("{:.1} GB", size / GIB)
    } else if size >= MIB {
        format!("{:.1} MB", size / MIB)
    } else if size >= KIB {
        format!("{:.1} KB", size / KIB)
    } else {
        format!("{size:.0} B")
    }
}
