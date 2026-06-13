# StreamWeave

StreamWeave 是一个本机 HLS 视频下载器。前端使用 Vite + 原生 HTML/CSS/JS，桌面端和后端能力由 Tauri 2 与 Rust 提供，当前优先支持 macOS。

## 当前形态

- `src/`：前端源码，负责任务列表、任务详情、设置页、Tauri 命令调用和事件监听。
- `src-tauri/`：Rust 后端，负责任务队列、网页提取、m3u8 解析、aria2c 下载、AES-128 HLS 解密、ffmpeg 合并和 ffprobe 验证。
- `scripts/`：Playwright 网页网络请求捕获脚本。
- `prd/`：产品需求、交互说明、技术方案和测试计划。
- `dist/`、`src-tauri/target/`：构建产物，不要手工修改。

## 应用场景

StreamWeave 面向需要批量保存普通 HLS 视频资源的本机工作流，适合以下场景：

- 下载用户自己有权访问的 `.m3u8` 视频，并自动合并为本地 mp4 文件。
- 从普通视频网页中提取网络请求里的 m3u8 地址，减少手动打开开发者工具查找播放列表的操作。
- 批量下载课程、会议回放、内部培训、公开视频归档等合法授权内容。
- 同时管理多个下载任务，保留每个任务的进度、日志、失败原因和临时文件，方便后续重试或排查。
- 对需要登录、验证码或手动播放触发的视频网页，使用应用自己的浏览器 Profile 进行一次性登录并复用会话。
- 在本机环境中集中管理 ffmpeg、ffprobe、aria2c、Node 和 Playwright 相关能力，避免每次手工拼接命令。

StreamWeave 不适合也不会支持 DRM 绕过、凭据抓取、读取系统浏览器 Cookie，或下载用户没有授权访问的内容。

## 功能

- 多任务下载队列，默认最多同时运行 3 个任务，可在设置中调整，最大 8 个。
- 任务级状态、进度、日志、暂停/继续、取消、重试和删除记录。
- 左侧任务列表，右侧任务详情；日志按任务隔离。
- 任务历史持久化，应用重启后保留任务列表和日志。
- 直接下载 `.m3u8`，或从普通视频网页提取 m3u8。
- 静态 HTML 扫描 + Playwright 网络捕获两级提取。
- Playwright 浏览器模式可选：无头浏览器、后台窗口、可见窗口。
- master playlist 自动选择最高质量 variant。
- aria2c 分片下载，ffmpeg 合并 mp4，ffprobe 验证输出。
- 合并前抽样检查分片是否包含有效 video/audio 流，提前提示分片异常。
- 全局解密并行槽位，默认 4，最大 16。
- 设置页支持下载目录选择、工具路径、附加参数、并发数和维护操作。
- 维护页支持检测 ffmpeg、ffprobe、aria2c、Node、Homebrew，并通过 Homebrew 安装 ffmpeg、aria2、Node。
- Dock 角标显示运行中 + 队列中任务总数；无任务时不显示角标。

## 依赖

系统工具：

```bash
brew install ffmpeg aria2 node
```

开发环境：

- Node.js / npm
- Rust / Cargo
- Tauri CLI
- Playwright Chromium
- ffmpeg / ffprobe
- aria2c

安装项目依赖：

```bash
cd "/Users/uncle/Developer/video downloader"
npm install
npx playwright install chromium
```

如果未安装 Rust：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## 运行

```bash
cd "/Users/uncle/Developer/video downloader"
source "$HOME/.cargo/env"
npm run tauri dev
```

前端开发服务：

```bash
npm run dev
```

构建前端：

```bash
npm run build
```

检查 Rust 后端：

```bash
cd src-tauri
source "$HOME/.cargo/env"
cargo check
```

## 自动打包

仓库包含 GitHub Actions 打包流程：

```text
.github/workflows/package.yml
```

触发方式：

- 推送到 `main` 分支自动打包，并在 workflow 运行结果中生成 artifact。
- 在 GitHub Actions 页面手动运行 `Package` workflow。
- 推送 `v*` tag 自动打包并创建 GitHub Release，例如：

```bash
git tag v0.1.0
git push origin v0.1.0
```

tag 构建会自动把应用版本同步为 tag 去掉 `v` 后的版本号。例如 `v0.2.0` 会打包出内部版本为 `0.2.0` 的应用；这个同步只发生在 CI 构建环境中，不会反写仓库文件。

当前 workflow 会构建 macOS 包：

- `macos-13`：Intel `x86_64-apple-darwin`
- `macos-14`：Apple Silicon `aarch64-apple-darwin`

每次构建都会上传 workflow artifact；tag 构建还会把 `.dmg` 和 `.app.zip` 上传到 GitHub Release。连续推送同一个分支或 tag 时，会自动取消该 ref 上仍在运行的旧打包任务。当前未配置 Apple Developer ID 签名和公证，因此产物是未签名包。

## 使用方式

主页面输入 URL 后点击“加入队列”：

- 输入 `.m3u8` URL：跳过网页提取，直接解析、下载、合并、验证。
- 输入普通视频网页 URL：先静态扫描 HTML；找不到可用 m3u8 时启动 Playwright 捕获网络请求。

网页解析流程：

1. 请求网页 HTML 并扫描 `.m3u8` 候选。
2. 校验候选 m3u8。
3. 静态扫描失败后启动 Playwright Chromium 捕获网络请求。
4. 捕获 `.m3u8` 请求或响应正文。
5. 校验候选 playlist。
6. 如为 master playlist，选择最高质量 variant。
7. 生成分片列表，使用 aria2c 下载。
8. 如 playlist 声明标准 AES-128 HLS 加密，则解密分片。
9. 合并前抽样用 ffprobe 检查分片媒体流。
10. 使用 ffmpeg 合并 mp4，并用 ffprobe 验证最终文件。

## 设置

设置页包含以下分组：

- 下载：下载目录选择、同时下载任务数、全局解密并行数。
- 网页解析：浏览器模式。
- 工具路径：ffmpeg、ffprobe、aria2c、Node 路径。
- 命令参数：aria2 和 ffmpeg 附加参数。
- 维护：工具检测、Homebrew 状态、安装 ffmpeg/aria2/Node、维护日志。

浏览器模式：

- 无头浏览器：不显示窗口，干扰最少，但部分网站会识别无头环境而不生成 m3u8。
- 后台窗口：启动有界面 Chromium，但尽量放到屏幕外，减少弹窗打断。
- 可见窗口：正常显示浏览器窗口，适合需要登录、验证码、手动点击播放或观察页面状态的网页。

## 数据目录

当前应用名和 bundle identifier：

```text
StreamWeave
com.uncle.streamweave
```

应用数据目录：

```text
~/Library/Application Support/StreamWeave
```

主要数据：

- `browser-profile/`：应用自己的 Playwright 持久浏览器 Profile。
- `tasks/`：任务临时工作目录，包含 m3u8、aria2 列表、concat 列表和分片。
- `task-records/`：按任务拆分的任务元数据。
- `task-logs/`：按任务拆分的日志。
- `tasks.json`：任务索引。

需要登录态的网页使用应用自己的持久 Profile。第一次遇到需要登录的网站时，使用可见窗口登录；后续会复用该 Profile。应用不直接读取 Chrome/Safari Cookie。

## 任务排序

任务列表按分组和时间降序排序：

1. 未完成任务：按添加时间降序。
2. 失败 / 已取消任务：按活动时间降序。
3. 已完成任务：按完成时间降序。

任务卡片会显示对应时间类型，例如“添加时间”、“活动时间”或“完成时间”。

## 日志与排查

下载任务通过 Tauri 事件实时更新：

- `download-task-updated`
- `download-log`
- `download-stage`
- `download-progress`
- `download-task-directory`
- `download-completed`
- `download-failed`
- `download-task-deleted`

任务日志会记录：

- 静态 HTML 响应状态和扫描结果。
- Playwright 捕获阶段日志。
- 候选 m3u8 和请求头摘要。
- playlist 校验结果。
- aria2c、ffmpeg、ffprobe 输出。
- 解密槽位等待提示。
- 合并前分片媒体流检查结果。

常见失败方向：

- m3u8 token 过期或分片 URL 过期。
- 站点需要登录、验证码或手动点击播放。
- 分片请求缺少 Referer、Origin、Cookie 或 User-Agent。
- playlist 没有声明标准 HLS key，但分片实际是私有混淆数据。
- 分片下载到错误响应、空文件或不可识别媒体流。

## 安全边界

本项目只处理用户可访问网页中的普通 HLS 下载流程，不加入：

- DRM 绕过。
- 凭据抓取。
- 直接读取 Chrome/Safari Cookie。
- 破解私有加密或非标准混淆视频流。

## 许可证

本项目以 MIT License 开源，详见 [LICENSE](LICENSE)。

## 常用验证

```bash
npm run build
cd src-tauri
source "$HOME/.cargo/env"
cargo check
```

启动 GUI：

```bash
cd "/Users/uncle/Developer/video downloader"
source "$HOME/.cargo/env"
npm run tauri dev
```
