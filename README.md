# 课业倒计时 / StudyDeadline

A lightweight, cross-platform desktop floating window for tracking study deadlines.

轻量桌面学习任务悬浮窗：登录雨课堂，定时只读拉取当前学期的作业/考试截止时间，
在置顶小窗中显示倒计时，并提供托盘、设置、日志、系统通知和本机加密会话。

> **非官方工具，请先阅读使用边界。**
> 本项目只面向用户本人账号的作业/考试**元数据**（标题、课程、时间、状态、链接），
> 不读取题目/答案/试卷，不提交作业/考试，不签到，不绕过验证码或风控。
> 雨课堂没有公开学生 API，用户协议明确禁止抓取、反向工程和绕过技术限制；
> 使用本项目仍可能导致账号受限或接口随时失效。请自行评估并承担风险。
> 首发仅建议个人自用，未经评估不要用于商业化或对外提供服务。

## 当前状态

| 项目 | 状态 |
|---|---|
| Linux 桌面 | 已在本机编译、安装、真实登录和任务拉取验证 |
| Windows 10/11 | 代码保持 Tauri 跨平台，但尚未在 Windows 实机发布验收 |
| macOS | 路线图；需要 Apple 签名/公证 |
| 雨课堂适配器 | 当前为内置 Rust 适配器 |
| 外部 SDK / 签名热更新 | 规范已编写，运行时尚未实现，见 [适配器 SDK 规范](docs/adapter-sdk.md) |
| 任务点击 | 使用系统浏览器打开课程详情；不注入/共享系统浏览器 Cookie |

## 功能

- **悬浮窗**：无边框、置顶、可拖动、可隐藏到托盘，按截止时间排序，逐秒倒计时
- **雨课堂登录**：内嵌 WebView 手动扫码/学校 SSO，支持标准、荷塘、长江、黄河入口
- **会话安全**：只读取 `yuketang.cn` 域 Cookie；AES-256-GCM 加密会话，主密钥保存在 OS 凭据库
- **只读拉取**：当前学期课程列表 → 课程进度树 → 作业/考试/测验截止时间；考试预告去重
- **提醒**：24h / 6h / 1h / 10m 阈值，系统通知；失败时保留旧任务并按刷新间隔重试
- **托盘**：显示/隐藏、立即刷新、设置、退出
- **设置**：刷新间隔、系统通知、开机启动（仅发布版）、窗口置顶、账号管理、日志查看
- **工程能力**：SQLite 缓存、敏感字段脱敏日志、单实例、关闭到托盘、Linux deb 打包

## 截图

暂无公开截图。运行的紧凑窗口包含：

- 顶部：应用名、同步状态、刷新/设置/最小化/隐藏
- 中部：按截止时间排序的任务卡片（平台、课程、类型、标题、时间、倒计时）
- 底部：只读提醒说明

## 快速开始

### 环境要求

- Node.js 22+
- pnpm 10+
- Rust stable
- Linux 还需安装 WebKitGTK/AppIndicator 开发包：

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf \
  libxdo-dev
```

### 开发模式

```bash
pnpm install
pnpm dev
```

开发模式依赖本地 Vite（`localhost:1420`）。**不要给开发版开启开机启动**：
开发版启动时依赖 Vite 服务，电脑重启后 Vite 不在，会显示
`Could not connect to localhost`。应用已对开发版禁用该选项。

### 构建并安装 Linux 正式版

```bash
cd apps/desktop
pnpm tauri build --bundles deb
sudo apt-get install -y ./src-tauri/target/release/bundle/deb/StudyDeadline_0.1.0_amd64.deb
```

安装后从应用菜单启动 **StudyDeadline**，或执行 `study-deadline`。
正式版使用嵌入的前端资源，不依赖 Vite；此时才可以在设置中安全开启开机启动。

### Windows / macOS

代码和 Tauri 配置保持跨平台，但尚未提供验证过的安装包。Windows/macOS 构建必须在
对应系统的 runner 或实机上完成，不能把 Linux 交叉构建作为发布产物。欢迎提交构建验证 PR。

## 使用说明

1. 首次启动阅读风险提示，勾选“仅本人账号、只读元数据”，选择雨课堂入口。
2. 打开登录窗口，完成二维码或学校 SSO；保持登录窗口打开。
3. 回到主窗口点击“我已完成登录”，等待任务拉取。
4. 点击任务卡片会在**系统浏览器**打开课程详情页。
   - 应用内 WebView 的登录态不会导出到系统浏览器；
   - 如果浏览器本身未登录雨课堂，需要在浏览器内自行登录一次。
5. 关闭主窗口默认隐藏到托盘；通过托盘菜单重新显示或退出。

### 任务页 URL

课程详情页使用 `classroom_id`，不是 `course_id`：

```text
{base}/v2/web/studentLog/{classroom_id}
```

这是 2026-11-04 根据雨课堂前端路由确认并实际处理的路径。平台改版后可能变化。

## 架构

```text
Svelte 5 单窗口（紧凑悬浮 / 设置）
        │ typed IPC + events
        ▼
Rust / Tauri 2
├─ commands      登录、刷新、设置、打开任务、日志
├─ auth          WebView 登录窗口 + Cookie 提取
├─ state         AppState / 会话 / 同步状态
├─ storage       SQLite（任务、账号、设置、通知）
├─ secret        OS 凭据库主密钥 + AES-256-GCM
├─ yuketang      雨课堂内置适配器、限速、解析、去重
├─ scheduler     定时刷新 + 分钟级提醒检查
├─ notifications 通知阈值去重
├─ tray          托盘菜单与 tooltip
└─ refresh       刷新编排、错误/部分数据保留
```

详细模块、数据流和安全边界见 [架构说明](docs/architecture.md)。

## 数据与安全

- 不保存账号密码；登录由用户在 WebView 中完成。
- 会话 Cookie 只从 `yuketang.cn` 域读取；加密后存 SQLite，主密钥在 OS 凭据库。
- 日志不记录 Cookie、Authorization、CSRF token、OAuth code/state 或响应体。
- 任务链接只允许 `https://yuketang.cn` / `*.yuketang.cn`，由系统浏览器打开。
- 默认 30 分钟刷新；串行请求课程树，每门课至少间隔 1–2 秒。
- 数据仅在用户本机；没有遥测、云同步或远程账号服务。

Linux 数据位置：

```text
~/.local/share/dev.donggu.studydeadline/
├─ study-deadline.sqlite3
├─ logs/app.log
└─ WebView 数据
```

## 已知限制

- 平台没有公开学生 API；接口、字段和登录流程可能在任何时候变化。
- 仅内置雨课堂适配器；当前添加平台需要改 Rust 代码并重新发版。
- 任务链接打开系统浏览器；跨应用共享 Cookie 不做，也不建议通过浏览器扩展窃取会话。
- 考试子域有独立会话；当前不读取考试内容，只展示主站可获得的考试元数据。
- Linux Wayland 下置顶/绝对定位受合成器限制；本项目主要验证 X11。
- Windows 通知、开机启动、托盘和多显示器尚未完成正式验收。
- 系统通知在 Linux 上依赖桌面通知服务；安装/权限差异可能导致不显示。

## Roadmap

- [x] 雨课堂内置适配器（课程、作业、考试、截止时间）
- [x] 托盘、倒计时、设置、加密会话、日志
- [x] Linux deb 发布构建
- [ ] Windows 实机构建与通知/开机启动验收
- [ ] 外部适配器 SDK：manifest、权限、QuickJS 受控运行时
- [ ] 签名包安装、版本兼容、回滚和远程目录
- [ ] 任务叶子深链（减少从课程页再跳转）
- [ ] macOS 签名/公证与 Linux Wayland 适配

## 文档

- [雨课堂平台接口文档](docs/platform/yuketang.md)
- [适配器 SDK 规范（含现状与路线图）](docs/adapter-sdk.md)
- [架构说明](docs/architecture.md)
- [Phase 0 验证记录](docs/phase0-findings.md)
- [参与贡献](CONTRIBUTING.md)
- [安全政策](SECURITY.md)

## 贡献

欢迎 Issues 和 PR，但请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 和
[SECURITY.md](SECURITY.md)。不要提交真实 Cookie、账号信息、课程数据或未脱敏的接口响应。

## License

MIT。见 [LICENSE](LICENSE)。
