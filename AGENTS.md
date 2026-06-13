# Repository Guidelines

## 项目结构与模块组织

本项目是本机 HLS 视频下载器，前端使用 Vite，桌面端和后端能力由 Tauri 2 与 Rust 提供。

- `src/`：前端源码。`main.js` 负责 UI 事件、Tauri 命令调用和下载进度事件监听；`styles.css` 负责界面样式。
- `src-tauri/`：Rust 后端。`src/lib.rs` 包含下载编排、m3u8 提取、aria2c/ffmpeg 调用和 Tauri 命令；`src/main.rs` 启动应用；`tauri.conf.json` 保存应用配置。
- `scripts/`：辅助脚本，目前包含基于 Playwright 的 m3u8 提取工具。
- `prd/`：产品需求、交互说明、技术方案和测试验收计划。
- `dist/`、`src-tauri/target/`：构建产物，不要手工修改。

## 构建、测试与开发命令

安装项目依赖：

```bash
npm install
npx playwright install chromium
```

安装系统工具：

```bash
brew install ffmpeg aria2
```

常用命令：

- `npm run dev`：在 `127.0.0.1:1420` 启动 Vite 前端。
- `npm run tauri dev`：启动桌面应用，联调 Rust 后端和前端。
- `npm run build`：构建前端到 `dist/`。
- `cd src-tauri && cargo check`：检查 Rust 后端类型和编译问题。

## 编码风格与命名约定

前端使用 ES modules、两空格缩进，优先使用 `const`，变量和函数使用 `camelCase`。DOM 查询集中放在 `src/main.js` 顶部，交互逻辑拆成小型辅助函数。

Rust 代码遵循 `rustfmt`，函数和字段使用 `snake_case`，结构体使用 PascalCase。Tauri 命令边界使用明确的 `Result<_, String>` 错误返回；耗时任务放到后台线程，并通过 Tauri 事件回传状态。

## 测试指南

当前没有正式自动化测试套件。提交前至少运行 `npm run build` 和 `cd src-tauri && cargo check`。涉及下载流程时，手动验证直接 `.m3u8` 输入和普通视频网页提取。修改提取逻辑时，在验证说明中附上静态 HTML 扫描和 Playwright 网络捕获日志。

## 提交与 Pull Request 规范

当前工作树没有可用 Git 历史，因此提交信息使用清晰的祈使句，例如 `Add download cancellation guard` 或 `Fix m3u8 variant selection`。PR 应说明用户可见变化、列出验证命令、标明依赖工具（`ffmpeg`、`aria2c`、Playwright Chromium），涉及 UI 或下载流程时附截图或关键日志。

## 安全与配置提示

不要加入 DRM 绕过、凭据抓取或直接读取 Chrome/Safari Cookie 的逻辑。需要登录的网站使用应用自己的持久浏览器 Profile：`~/Library/Application Support/video-downloader/browser-profile`。
