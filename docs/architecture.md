# 架构说明

本文描述当前 `v0.1.0` 的实际实现，以及后续适配器 SDK 的接入位置。
产品边界和风险说明见根目录 [README](../README.md)，平台接口见
[雨课堂接口文档](platform/yuketang.md)。

## 1. 总览

```text
Svelte 5 单 WebView
  ├─ 紧凑悬浮视图
  └─ 设置视图
        │ Tauri invoke / event
        ▼
Rust Core
  ├─ commands         Tauri 命令与登录窗口
  ├─ state            全局状态、Dashboard、同步状态
  ├─ refresh          拉取编排与部分失败策略
  ├─ scheduler        3 秒首刷 + 分钟级提醒循环
  ├─ yuketang         内置平台适配器
  ├─ storage          SQLite 任务/账号/设置/通知
  ├─ secret           keyring 主密钥 + AES-256-GCM 会话
  ├─ notifications    阈值去重与系统通知
  ├─ tray             托盘菜单与 tooltip
  └─ models           共享任务/设置/账号 DTO
        │
        ▼
平台只读 Web API（Cookie 鉴权）
```

## 2. 启动与登录

1. Tauri 启动，初始化日志（`app_log_dir/app.log`）。
2. `AppState::initialize` 打开 SQLite、读取设置、读取账号行。
3. 从 OS 凭据库取 32 字节主密钥，解密账号会话；凭据库不可用时会话只在内存中。
4. 创建托盘。托盘创建失败时会关闭“跳过任务栏”，避免窗口无法找回。
5. 启动调度器；若已有有效会话，约 3 秒后自动刷新。
6. 用户点击登录时，打开独立登录 WebView（label `login-yuketang`）：
   - 只允许 HTTPS；
   - 不匹配任何应用 capability，远程页面无法调用应用命令；
   - 登录成功后，用 Tauri Rust Cookie API 在窗口存活时读取会话 Cookie。
7. 用一次只读课程列表请求验证会话；成功后才保存账号、加密会话和任务。

会话加密：

- 明文：`Session` JSON（base URL、region、Cookie 头、CSRF、获取时间）；
- 主密钥：OS keyring（Linux Secret Service / macOS Keychain / Windows Credential Manager）；
- 密文：AES-256-GCM（12 字节随机 nonce + 密文）存入 SQLite。

## 3. 数据与任务

SQLite 表：

| 表 | 内容 |
|---|---|
| `settings` | 单行 JSON 设置 |
| `accounts` | 账号、区域、加密会话、上次成功/错误 |
| `tasks` | 归一化任务快照 |
| `notifications` | `(task_id, threshold_minutes)` 去重 |

任务稳定 ID：`yuketang:<account>:leaf:<course_id>:<leafinfo_id>`。
考试任务使用 `exam:<id>`。ID 只用于本地主键，平台侧去重另用
`(classroom_id, title, start, due)`。

完整快照会删除该账号未返回的旧任务；部分失败（`complete=false`）只 upsert，
保留旧任务。任务不会因为“未返回”被推断为已经提交。

## 4. 刷新与提醒

- 默认刷新间隔 30 分钟；可选 15/30/60/120 分钟。
- 调度器每 60 秒检查一次：
  - 到刷新时间且有会话 → 拉取；
  - 检查提醒阈值 → 发送通知；
  - 更新托盘 tooltip。
- 雨课堂适配器：
  - 取当前学期（最大有效 `term`）课程；
  - 串行拉取每个课程的 `c27` 进度树，每门课间至少 1–2 秒；
  - 只纳入 `is_score && score_deadline > 0` 的叶子；
  - 考试预告接口（秒）统一转为毫秒，并与叶子去重；
  - 401/403/3xx 视为会话失效，停止本轮并提示重新登录；
  - 单课程失败返回 `PARTIAL`，保留旧任务。
- 通知阈值默认 `[1440, 360, 60, 10]` 分钟；发送后标记当前和更大的阈值，
  避免应用启动晚时补发一堆通知。

## 5. 任务打开

当前版本用系统浏览器打开：

```text
{base}/v2/web/studentLog/{classroom_id}
```

- 应用内 WebView 的 Cookie 不导出到系统浏览器；
- 如果浏览器未登录平台，需要用户自行登录；
- 不通过扩展、调试接口或读取浏览器数据库来共享 Cookie。

后续可选方案是应用内任务 WebView（与登录窗口共享 cookie store），但它必须
配合正确的 `classroom_id` URL，并在设置中显式选择。

## 6. 日志与隐私

- `tracing` 输出到 `app.log`，当前无每日轮转；建议发布前补上轮转/保留策略。
- 不记录 Cookie、Authorization、CSRF、OAuth code/state、请求/响应体。
- 适配器不支持任意日志字段；宿主可做统一脱敏。
- 无遥测、无云同步、无远程账号服务。

## 7. 安全边界

- 登录 WebView：无应用 capability、HTTPS 限定；
- 前端：只有主窗口有 capability；任务标题按文本渲染；
- 任务 URL：HTTPS + `yuketang.cn` 域校验后才交给系统浏览器；
- 会话：OS keyring 主密钥 + AES-GCM；凭据库不可用时仅内存；
- 适配器：当前为内置 Rust；未来外部 SDK 计划使用 QuickJS 受控运行时 + 域名白名单 +
  Ed25519 签名。QuickJS 不是敌对代码强沙箱，第三方不受信代码应使用 WASM/独立进程。

## 8. 当前不足与演进

1. **SDK 未落地**：没有动态插件加载、签名目录和版本回滚；新增平台仍需改 Rust。
2. **Windows/macOS 未验收**：代码跨平台，但只在 Linux X11 实机上传过完整链路。
3. **任务深链**：目前稳定到课程详情页，尚未生成具体叶子/作业深链。
4. **系统浏览器不共享会话**：跨应用 Cookie 共享不做，避免安全风险和浏览器差异。
5. **日志轮转、SQLite 备份/修复**：尚未完成生产级完善。
6. **自动更新**：未实现；适配器热更新更是之后的里程碑。

适配器迁移路线：

```text
内置 Rust（当前）
  → 统一 adapter manifest + 包格式
  → QuickJS 受控运行时 + 本地安装
  → Ed25519 签名 + 版本兼容 + 回滚
  → 远程签名目录 + 第二个平台
```
